use std::{
    fmt,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use reqwest::{
    Url,
    blocking::{Client, RequestBuilder, Response},
    cookie::Jar,
    header::LOCATION,
    redirect::Policy,
};
use serde::{Deserialize, Serialize};

pub const VPN_PORTAL: &str = "https://vpn.neuedu.com";
pub const XK_ORIGIN: &str = "https://xk.neusoft.edu.cn";
pub const XK_LOGIN: &str = "https://xk.neusoft.edu.cn/xsxk/profile/index.html";
pub const VPN_GUIDANCE: &str = "选课网关要求本程序完成 VPN 验证（不代表 aTrust 没有连接）。\n如果普通浏览器能访问，请点击“VPN 浏览器授权”，在独立窗口完成学校认证，再点击窗口中的“完成授权并检测”。\n仅打开外部浏览器不会把认证状态传给本程序；校内也可直接连接校园网后重试。";

// Only the dedicated authorization window supplies these host-scoped cookies.
// They are never written to preferences, logs, command-line arguments or disk.
static VPN_SESSION: Mutex<Vec<SessionCookie>> = Mutex::new(Vec::new());

#[derive(Clone, Serialize, Deserialize)]
pub struct SessionCookie {
    pub name: String,
    pub value: String,
    pub path: String,
}

impl SessionCookie {
    fn header(&self) -> Result<String> {
        if self.name.is_empty()
            || self
                .name
                .bytes()
                .any(|b| !b.is_ascii_alphanumeric() && !b"!#$%&'*+-.^_`|~".contains(&b))
            || self
                .value
                .bytes()
                .any(|b| b <= 0x20 || b >= 0x7f || b"\";,\\".contains(&b))
            || !self.path.starts_with('/')
            || self
                .path
                .bytes()
                .any(|b| !(0x20..0x7f).contains(&b) || b == b';')
            || self.name.len() + self.value.len() + self.path.len() > 16_384
        {
            bail!("授权会话数据格式无效");
        }
        // Omit Domain: imported cookies can only be sent to the exact XK host.
        Ok(format!(
            "{}={}; Path={}; Secure; HttpOnly",
            self.name, self.value, self.path
        ))
    }
}

pub fn session_jar(cookies: &[SessionCookie]) -> Result<Arc<Jar>> {
    if cookies.len() > 128 {
        bail!("授权会话数据过多");
    }
    let jar = Arc::new(Jar::default());
    for cookie in cookies {
        add_session_cookie(&jar, cookie)?;
    }
    Ok(jar)
}

pub fn add_session_cookie(jar: &Jar, cookie: &SessionCookie) -> Result<()> {
    jar.add_cookie_str(&cookie.header()?, &Url::parse(XK_ORIGIN)?);
    Ok(())
}

pub fn current_session_jar() -> Result<Arc<Jar>> {
    session_jar(
        &VPN_SESSION
            .lock()
            .map_err(|_| anyhow::anyhow!("授权会话暂不可用"))?,
    )
}

pub fn install_session(cookies: Vec<SessionCookie>) -> Result<()> {
    session_jar(&cookies)?;
    *VPN_SESSION
        .lock()
        .map_err(|_| anyhow::anyhow!("授权会话暂不可用"))? = cookies;
    Ok(())
}

#[derive(Debug)]
pub struct VpnRequired;

impl fmt::Display for VpnRequired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(VPN_GUIDANCE)
    }
}

impl std::error::Error for VpnRequired {}

pub fn is_vpn_url(url: &Url) -> bool {
    url.host_str() == Some("vpn.neuedu.com")
}

#[cfg(test)]
pub fn build_client() -> Result<Client> {
    build_client_with_jar(current_session_jar()?)
}

pub fn build_client_with_jar(jar: Arc<Jar>) -> Result<Client> {
    Client::builder()
        .cookie_provider(jar)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        // A 307 can forward the original POST body. Never forward it to a
        // different origin, even if reqwest strips the Authorization header.
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 {
                return attempt.error("重定向次数过多");
            }
            if attempt.previous().first().is_some_and(|original| {
                original.origin() == attempt.url().origin()
            }) {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/150.0.0.0 Safari/537.36")
        .build()
        .context("HTTP 客户端初始化失败")
}

pub fn send_checked(request: RequestBuilder, stage: &str) -> Result<Response> {
    let response = request
        .send()
        .map_err(reqwest::Error::without_url)
        .with_context(|| format!("{stage}请求失败；请检查校园网/VPN连接"))?;
    let destination = response
        .headers()
        .get(LOCATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|location| response.url().join(location).ok());
    if is_vpn_url(response.url()) || destination.as_ref().is_some_and(is_vpn_url) {
        return Err(VpnRequired.into());
    }
    if response.status().is_redirection() {
        bail!(
            "{stage}收到未允许的跳转（HTTP {}），请检查校园网/VPN；未向跳转地址发送数据",
            response.status().as_u16()
        );
    }
    if !response.status().is_success() {
        // Do not display signed gateway URLs, query tokens, or response bodies.
        bail!("{stage}返回错误状态：HTTP {}", response.status().as_u16());
    }
    Ok(response)
}

pub trait CheckedRequest {
    fn send_checked(self, stage: &str) -> Result<Response>;
}

impl CheckedRequest for RequestBuilder {
    fn send_checked(self, stage: &str) -> Result<Response> {
        send_checked(self, stage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::cookie::CookieStore;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        thread,
    };

    fn server(responses: Vec<String>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            for response in responses {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&mut socket);
                let mut content_length = 0;
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        content_length = value.trim().parse::<usize>().unwrap();
                    }
                }
                reader.read_exact(&mut vec![0; content_length]).unwrap();
                socket.write_all(response.as_bytes()).unwrap();
            }
        });
        (url, handle)
    }

    #[test]
    fn vpn_redirect_is_detected_without_replaying_or_disclosing_token() {
        for status in [302, 307] {
            let (url, handle) = server(vec![format!(
                "HTTP/1.1 {status} Redirect\r\nLocation: https://vpn.neuedu.com:4443/controller/v1/public/verify?t=TEST_SECRET\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )]);
            let error = send_checked(
                build_client().unwrap().post(url).body("password=test"),
                "验证码接口",
            )
            .unwrap_err();
            assert!(error.is::<VpnRequired>());
            assert!(!format!("{error:#}").contains("TEST_SECRET"));
            handle.join().unwrap();
        }
    }

    #[test]
    fn cross_origin_307_never_forwards_post_body() {
        let destination = TcpListener::bind("127.0.0.1:0").unwrap();
        destination.set_nonblocking(true).unwrap();
        let (url, handle) = server(vec![format!(
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{}/verify?t=TEST_SECRET\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            destination.local_addr().unwrap()
        )]);
        let error = send_checked(
            build_client().unwrap().post(url).body("password=test"),
            "登录接口",
        )
        .unwrap_err();
        assert!(error.to_string().contains("未允许的跳转"));
        assert!(!error.to_string().contains("TEST_SECRET"));
        assert_eq!(
            destination.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        handle.join().unwrap();
    }

    #[test]
    fn same_origin_redirect_still_works() {
        let (url, handle) = server(vec![
            "HTTP/1.1 302 Found\r\nLocation: /ready\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned(),
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK".to_owned(),
        ]);
        assert_eq!(
            send_checked(build_client().unwrap().get(url), "测试")
                .unwrap()
                .text()
                .unwrap(),
            "OK"
        );
        handle.join().unwrap();
    }

    #[test]
    fn direct_401_is_not_misreported_as_vpn_or_leaks_query() {
        let (url, handle) = server(vec![
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_owned(),
        ]);
        let error = send_checked(
            build_client().unwrap().get(format!("{url}/?t=TEST_SECRET")),
            "登录接口",
        )
        .unwrap_err();
        assert!(!error.is::<VpnRequired>());
        assert!(error.to_string().contains("401"));
        assert!(!error.to_string().contains("TEST_SECRET"));
        handle.join().unwrap();
    }

    #[test]
    fn vpn_host_matching_is_exact() {
        assert!(is_vpn_url(
            &Url::parse("https://vpn.neuedu.com:4443/verify").unwrap()
        ));
        assert!(!is_vpn_url(
            &Url::parse("https://vpn.neuedu.com.example.com/").unwrap()
        ));
    }

    #[test]
    fn imported_vpn_cookie_survives_adding_course_authorization() {
        let jar = session_jar(&[SessionCookie {
            name: "vpn_session".into(),
            value: "dummy-vpn".into(),
            path: "/".into(),
        }])
        .unwrap();
        add_session_cookie(
            &jar,
            &SessionCookie {
                name: "Authorization".into(),
                value: "dummy-course".into(),
                path: "/".into(),
            },
        )
        .unwrap();
        let header = jar.cookies(&Url::parse(XK_LOGIN).unwrap()).unwrap();
        let header = header.to_str().unwrap();
        assert!(header.contains("vpn_session=dummy-vpn"));
        assert!(header.contains("Authorization=dummy-course"));
        for address in [
            "https://vpn.neuedu.com/",
            "https://other.neusoft.edu.cn/",
            "http://xk.neusoft.edu.cn/",
        ] {
            assert!(jar.cookies(&Url::parse(address).unwrap()).is_none());
        }
    }

    #[test]
    fn imported_cookie_path_is_preserved() {
        let jar = session_jar(&[SessionCookie {
            name: "restricted".into(),
            value: "dummy".into(),
            path: "/xsxk/auth".into(),
        }])
        .unwrap();
        assert!(
            jar.cookies(&Url::parse(&format!("{XK_ORIGIN}/xsxk/auth/captcha")).unwrap())
                .is_some()
        );
        assert!(jar.cookies(&Url::parse(XK_LOGIN).unwrap()).is_none());
    }

    #[test]
    fn cookie_header_injection_is_rejected_without_leaking_values() {
        for value in [
            "TEST_SECRET; Domain=evil.test",
            "TEST_SECRET\r\nOther: x",
            "TEST_SECRET\"value",
        ] {
            let cookie = SessionCookie {
                name: "vpn".into(),
                value: value.into(),
                path: "/".into(),
            };
            let error = session_jar(&[cookie]).err().unwrap();
            assert!(!error.to_string().contains("TEST_SECRET"));
        }
        for path in ["/; Domain=evil.test", "not-a-path", "/\n"] {
            let cookie = SessionCookie {
                name: "vpn".into(),
                value: "dummy".into(),
                path: path.into(),
            };
            assert!(session_jar(&[cookie]).is_err());
        }
    }
}
