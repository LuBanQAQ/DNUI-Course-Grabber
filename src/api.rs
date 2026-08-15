use std::{collections::HashMap, thread, time::Duration};

use aes::Aes128;
use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use cipher::{BlockEncryptMut, KeyInit, block_padding::Pkcs7};
use ecb::Encryptor;
use regex::Regex;
use reqwest::{
    blocking::{Client, Response},
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, COOKIE, ORIGIN, REFERER, USER_AGENT},
};
use serde_json::{Value, json};

use crate::{
    model::{Batch, Course},
    ocr::recognize_captcha,
};

const DEFAULT_HOST: &str = "https://xk.neusoft.edu.cn";
const DEFAULT_AES_KEY: &str = "MWMqg2tPcDkxcm11";

type Aes128EcbEnc = Encryptor<Aes128>;

#[derive(Clone)]
pub struct ApiClient {
    client: Client,
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
        let excerpt: String = text.chars().take(240).collect();
        anyhow!(
            "{stage}未返回 JSON：HTTP {status}，Content-Type={content_type}，响应={excerpt:?}，{error}"
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
        let host = DEFAULT_HOST.to_owned();
        let api = format!("{host}/xsxk");
        let profile = format!("{api}/profile");
        let client = Client::builder()
            .cookie_store(true)
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/150.0.0.0 Safari/537.36")
            .build()
            .context("HTTP 客户端初始化失败")?;
        Ok(Self {
            client,
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
            .send()
            .context("加载登录页失败")?
            .error_for_status()
            .context("登录页返回错误状态")?
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
                    .send()
                    .context("请求验证码失败")?
                    .error_for_status()
                    .context("验证码接口返回错误状态")?,
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
                    .send()
                    .context("登录请求失败")?
                    .error_for_status()
                    .context("登录接口返回错误状态")?,
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

    pub fn bind_batch(&self, token: &str, batch_id: &str) -> Result<String> {
        let form = [("batchId", batch_id)];
        let body = json_response(
            self.common(self.client.post(format!("{}/elective/user", self.api)))
                .header(AUTHORIZATION, token)
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .form(&form)
                .send()
                .context("进入所选轮次失败")?
                .error_for_status()
                .context("进入轮次接口返回错误状态")?,
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
        let html = self
            .common(self.client.get(format!(
                "{}/elective/grablessons?batchId={batch_id}",
                self.api
            )))
            .header(AUTHORIZATION, token)
            .header(COOKIE, format!("Authorization={token}"))
            .send()
            .context("加载选课页面失败")?
            .error_for_status()
            .context("选课页面返回错误状态")?
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
                    .send()
                    .with_context(|| format!("请求课程列表第 {page} 页失败"))?
                    .error_for_status()
                    .with_context(|| format!("课程列表第 {page} 页返回错误状态"))?,
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
            .send()
            .context("保活请求失败")?
            .error_for_status()
            .context("保活接口返回错误状态")?;
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
                .send()
                .context("提交选课请求失败")?
                .error_for_status()
                .context("选课接口返回错误状态")?,
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
                .send()
                .context("提交退课请求失败")?
                .error_for_status()
                .context("退课接口返回错误状态")?,
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
