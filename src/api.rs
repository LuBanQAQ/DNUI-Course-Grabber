use std::{collections::HashMap, sync::Arc, thread, time::Duration};

use aes::Aes128;
use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use cipher::{BlockEncryptMut, KeyInit, block_padding::Pkcs7};
use ecb::Encryptor;
use regex::Regex;
use reqwest::{
    blocking::{Client, Response},
    cookie::Jar,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, ORIGIN, REFERER, USER_AGENT},
};
use serde_json::{Value, json};

use crate::{
    model::{Batch, Course},
    network::{self, CheckedRequest, VpnRequired, build_client_with_jar, current_session_jar},
    ocr::recognize_captcha,
};

const DEFAULT_HOST: &str = "https://xk.neusoft.edu.cn";
const DEFAULT_AES_KEY: &str = "MWMqg2tPcDkxcm11";

type Aes128EcbEnc = Encryptor<Aes128>;

#[derive(Clone)]
pub struct ApiClient {
    client: Client,
    cookie_jar: Arc<Jar>,
    host: String,
    api: String,
    profile: String,
}

#[derive(Clone)]
pub struct LoginSession {
    pub api: ApiClient,
    pub token: String,
    pub batches: Vec<Batch>,
}

fn json_response(response: Response, stage: &str) -> Result<Value> {
    let status = response.status();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("未知")
        .to_owned();
    let text = response
        .text()
        .with_context(|| format!("{stage}读取响应失败"))?;
    serde_json::from_str(&text).map_err(|error| {
        if text.contains("vpn.neuedu.com") {
            return VpnRequired.into();
        }
        anyhow!(
            "{stage}未返回 JSON：HTTP {status}，Content-Type={content_type}，{error}；请检查校园网/VPN连接"
        )
    })
}

fn code(value: &Value) -> i64 {
    value
        .get("code")
        .and_then(Value::as_i64)
        .unwrap_or_default()
}

fn message(value: &Value) -> String {
    value
        .get("msg")
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("未知错误")
        .to_owned()
}

fn encrypt_password(password: &str, key: &str) -> Result<String> {
    let key_bytes = key.as_bytes();
    if key_bytes.len() != 16 {
        bail!("AES 密钥长度异常：{}，预期 16", key_bytes.len());
    }
    let encrypted = Aes128EcbEnc::new_from_slice(key_bytes)
        .context("AES 初始化失败")?
        .encrypt_padded_vec_mut::<Pkcs7>(password.as_bytes());
    Ok(STANDARD.encode(encrypted))
}

impl ApiClient {
    pub fn new() -> Result<Self> {
        Self::with_jar(current_session_jar()?)
    }

    fn with_jar(cookie_jar: Arc<Jar>) -> Result<Self> {
        let host = DEFAULT_HOST.to_owned();
        let api = format!("{host}/xsxk");
        let profile = format!("{api}/profile");
        let client = build_client_with_jar(cookie_jar.clone())?;
        Ok(Self {
            client,
            cookie_jar,
            host,
            api,
            profile,
        })
    }

    fn common(
        &self,
        builder: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        builder
            .header(ACCEPT, "application/json, text/plain, */*")
            .header(ORIGIN, &self.host)
            .header(REFERER, format!("{}/index.html", self.profile))
            .header(USER_AGENT, "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/150.0.0.0 Safari/537.36")
    }

    pub fn login(username: &str, password: &str, retries: usize) -> Result<LoginSession> {
        let api = Self::new()?;
        let profile_html = api
            .common(api.client.get(format!("{}/index.html", api.profile)))
            .send_checked("登录页")?
            .text()
            .context("读取登录页失败")?;
        let key = Regex::new(r#"loginVue\.loginForm\.aesKey\s*=\s*\"([^\"]+)\""#)?
            .captures(&profile_html)
            .and_then(|captures| captures.get(1))
            .map(|value| value.as_str())
            .unwrap_or(DEFAULT_AES_KEY);
        let encrypted = encrypt_password(password, key)?;

        let mut last_error = String::new();
        for attempt in 1..=retries.max(1) {
            let captcha = json_response(
                api.common(api.client.post(format!("{}/auth/captcha", api.api)))
                    .send_checked("验证码接口")?,
                "验证码接口",
            )?;
            if code(&captcha) != 200 {
                bail!("验证码接口失败：{}", message(&captcha));
            }
            let data = captcha.get("data").cloned().unwrap_or(Value::Null);
            let image = data
                .get("captcha")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let uuid = data.get("uuid").and_then(Value::as_str).unwrap_or_default();
            if image.is_empty() || uuid.is_empty() {
                bail!("验证码接口返回不完整")
            }
            let answer = recognize_captcha(image)?;
            let form = [
                ("loginname", username),
                ("password", encrypted.as_str()),
                ("captcha", answer.as_str()),
                ("uuid", uuid),
            ];
            let login = json_response(
                api.common(api.client.post(format!("{}/auth/login", api.api)))
                    .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .form(&form)
                    .send_checked("登录接口")?,
                "登录接口",
            )?;
            if code(&login) == 200 {
                let token = login
                    .pointer("/data/token")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if token.is_empty() {
                    bail!("登录成功但没有返回 token")
                }
                let student = login
                    .pointer("/data/student")
                    .cloned()
                    .unwrap_or(Value::Null);
                let mut batches = Vec::new();
                for (field, group) in [
                    ("electiveBatchList", "普通轮次"),
                    ("expElectiveBatchList", "实验课轮次"),
                ] {
                    for item in student
                        .get(field)
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        if let Ok(mut batch) = serde_json::from_value::<Batch>(item.clone()) {
                            batch.group = group.to_owned();
                            batches.push(batch);
                        }
                    }
                }
                if batches.is_empty() {
                    bail!("登录成功但未返回选课轮次")
                }
                return Ok(LoginSession {
                    api,
                    token,
                    batches,
                });
            }
            last_error = message(&login);
            let captcha_error =
                last_error.contains("验证码") || last_error.to_lowercase().contains("captcha");
            if !captcha_error || attempt == retries.max(1) {
                break;
            }
        }
        bail!("登录失败：{last_error}")
    }

    /// No credentials and no OCR: verify that this process can reach the actual
    /// captcha API, not merely a browser's authenticated VPN portal.
    pub fn check_network() -> Result<()> {
        Self::new()?.check_current_network()
    }

    pub fn authorize_vpn() -> Result<()> {
        let cookies = crate::vpn_auth::authorize()?;
        let candidate = Self::with_jar(network::session_jar(&cookies)?)?;
        candidate.check_current_network().map_err(|error| {
            if error.is::<VpnRequired>() {
                anyhow!("授权窗口已经返回，但桌面程序仍被 VPN 网关要求验证。该连接可能还依赖浏览器或应用级隧道；当前未标记为授权成功，请联系学校确认是否允许桌面客户端接入。")
            } else {
                error.context("授权后的接口检测未通过")
            }
        })?;
        network::install_session(cookies)
    }

    fn check_current_network(&self) -> Result<()> {
        let api = self;
        let body = json_response(
            api.common(api.client.post(format!("{}/auth/captcha", api.api)))
                .send_checked("网络检测/验证码接口")?,
            "网络检测/验证码接口",
        )?;
        if code(&body) != 200
            || body
                .pointer("/data/captcha")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        {
            bail!("网络检测未通过：选课验证码接口未返回有效数据，请检查校园网/VPN后重试");
        }
        Ok(())
    }

    pub fn bind_batch(&self, token: &str, batch_id: &str) -> Result<String> {
        let form = [("batchId", batch_id)];
        let body = json_response(
            self.common(self.client.post(format!("{}/elective/user", self.api)))
                .header(AUTHORIZATION, token)
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .form(&form)
                .send_checked("进入轮次接口")?,
            "进入轮次接口",
        )?;
        if code(&body) != 200 {
            bail!("选择轮次失败：{}", message(&body));
        }
        Ok(body
            .pointer("/data/student/campus")
            .and_then(Value::as_str)
            .unwrap_or("1")
            .to_owned())
    }

    pub fn discover_teaching_class_type(&self, token: &str, batch_id: &str) -> Result<String> {
        // A manually supplied Cookie header would hide the VPN cookies in the jar.
        let auth_cookie = network::SessionCookie {
            name: "Authorization".to_owned(),
            value: token.to_owned(),
            path: "/".to_owned(),
        };
        network::add_session_cookie(&self.cookie_jar, &auth_cookie)?;
        let html = self
            .common(self.client.get(format!(
                "{}/elective/grablessons?batchId={batch_id}",
                self.api
            )))
            .header(AUTHORIZATION, token)
            .send_checked("选课页面")?
            .text()
            .context("读取选课页面失败")?;
        let captures = Regex::new(r#"(?s)grablessonsVue\.menuData\.menuList\s*=\s*(\[.*?\])\s*;"#)?
            .captures(&html)
            .ok_or_else(|| anyhow!("未能从选课页面识别课程类型"))?;
        let menus: Vec<Value> =
            serde_json::from_str(captures.get(1).unwrap().as_str()).context("课程菜单解析失败")?;
        menus
            .iter()
            .filter_map(|item| item.get("teachingClassType").and_then(Value::as_str))
            .find(|kind| !matches!(*kind, "YXKC" | "ALLKC"))
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("该轮次没有可抓取的课程菜单"))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn fetch_courses(
        &self,
        token: &str,
        batch_id: &str,
        teaching_class_type: &str,
        campus: &str,
        page_size: usize,
        interval_ms: u64,
        retry_403: usize,
        penalty_ms: u64,
    ) -> Result<(Vec<Course>, usize)> {
        let mut rows = Vec::new();
        let mut page = 1usize;
        let mut total = 0usize;
        let mut fetched_course_groups = 0usize;
        loop {
            let payload = json!({
                "teachingClassType": teaching_class_type,
                "pageNumber": page,
                "pageSize": page_size,
                "orderBy": "",
                "campus": campus,
            });
            let mut response_body = Value::Null;
            for attempt in 0..=retry_403 {
                response_body = json_response(
                    self.common(
                        self.client
                            .post(format!("{}/elective/clazz/list", self.api)),
                    )
                    .header(AUTHORIZATION, token)
                    .header("batchId", batch_id)
                    .header(CONTENT_TYPE, "application/json;charset=UTF-8")
                    .header(
                        REFERER,
                        format!("{}/elective/grablessons?batchId={batch_id}", self.api),
                    )
                    .json(&payload)
                    .send_checked(&format!("课程列表第 {page} 页"))?,
                    "课程列表接口",
                )?;
                if code(&response_body) == 200 {
                    break;
                }
                let msg = message(&response_body);
                if code(&response_body) == 403 && attempt < retry_403 {
                    let _ = self.heartbeat(token);
                    thread::sleep(Duration::from_millis(
                        penalty_ms.saturating_mul(attempt as u64 + 1),
                    ));
                    continue;
                }
                bail!("课程列表第 {page} 页失败：{msg}")
            }
            if code(&response_body) != 200 {
                bail!("课程列表第 {page} 页失败：{}", message(&response_body));
            }
            let data = response_body.get("data").cloned().unwrap_or(Value::Null);
            if page == 1 {
                total = data
                    .get("total")
                    .and_then(Value::as_u64)
                    .unwrap_or_default() as usize;
            }
            let page_rows = data
                .get("rows")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if page_rows.is_empty() {
                break;
            }
            fetched_course_groups += page_rows.len();
            rows.extend(page_rows.into_iter().flat_map(Course::expand_api_row));
            if fetched_course_groups >= total {
                break;
            }
            page += 1;
            thread::sleep(Duration::from_millis(interval_ms.max(1_500)));
        }
        let teaching_class_total = rows.len();
        Ok((rows, teaching_class_total))
    }

    pub fn heartbeat(&self, token: &str) -> Result<()> {
        self.common(self.client.post(format!("{}/web/now", self.api)))
            .header(AUTHORIZATION, token)
            .send_checked("保活接口")?;
        Ok(())
    }

    pub fn select_course(
        &self,
        token: &str,
        batch_id: &str,
        teaching_class_type: &str,
        course: &Course,
    ) -> Result<Value> {
        let mut form = HashMap::new();
        form.insert("clazzType", teaching_class_type);
        let clazz_id = course.id();
        let secret = course.secret();
        form.insert("clazzId", clazz_id.as_str());
        form.insert("secretVal", secret.as_str());
        let body = json_response(
            self.common(self.client.post(format!("{}/elective/clazz/add", self.api)))
                .header(AUTHORIZATION, token)
                .header("batchId", batch_id)
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(
                    REFERER,
                    format!("{}/elective/grablessons?batchId={batch_id}", self.api),
                )
                .form(&form)
                .send_checked("选课接口")?,
            "选课接口",
        )?;
        Ok(body)
    }

    pub fn drop_course(
        &self,
        token: &str,
        batch_id: &str,
        teaching_class_type: &str,
        course: &Course,
    ) -> Result<Value> {
        let clazz_id = course.id();
        let secret = course.secret();
        let form = [
            ("clazzType", teaching_class_type),
            ("clazzId", clazz_id.as_str()),
            ("secretVal", secret.as_str()),
        ];
        json_response(
            self.common(self.client.post(format!("{}/elective/clazz/del", self.api)))
                .header(AUTHORIZATION, token)
                .header("batchId", batch_id)
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(
                    REFERER,
                    format!("{}/elective/grablessons?batchId={batch_id}", self.api),
                )
                .form(&form)
                .send_checked("退课接口")?,
            "退课接口",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{ApiClient, DEFAULT_AES_KEY, encrypt_password};

    #[test]
    fn password_encryption_matches_python_version() {
        assert_eq!(
            encrypt_password("hello", DEFAULT_AES_KEY).unwrap(),
            "zTB/3Oiwdhio9uX5c1PYEA=="
        );
    }

    #[test]
    #[ignore = "read-only live network probe; requires the XK service"]
    fn live_network_probe_reports_access_or_vpn_requirement() {
        match ApiClient::check_network() {
            Ok(()) => println!("NETWORK_READY"),
            Err(error) if error.is::<crate::network::VpnRequired>() => println!("VPN_REQUIRED"),
            Err(error) => panic!("unexpected network result: {error:#}"),
        }
    }

    #[test]
    #[ignore = "requires XK_TEST_USERNAME and XK_TEST_PASSWORD; performs read-only live login/list"]
    fn live_login_and_course_list_are_non_empty() {
        let username = std::env::var("XK_TEST_USERNAME").unwrap();
        let password = std::env::var("XK_TEST_PASSWORD").unwrap();
        let session = ApiClient::login(&username, &password, 5).unwrap();
        let batch = session
            .batches
            .iter()
            .find(|batch| batch.can_select == "1")
            .unwrap();
        let campus = session.api.bind_batch(&session.token, &batch.code).unwrap();
        let class_type = session
            .api
            .discover_teaching_class_type(&session.token, &batch.code)
            .unwrap();
        let (courses, total) = session
            .api
            .fetch_courses(
                &session.token,
                &batch.code,
                &class_type,
                &campus,
                100,
                1_800,
                3,
                1_500,
            )
            .unwrap();
        assert!(total > 0);
        assert!(!courses.is_empty());
        assert!(courses.iter().all(|course| !course.id().is_empty()));
        assert!(courses.iter().all(|course| !course.secret().is_empty()));
    }
}
