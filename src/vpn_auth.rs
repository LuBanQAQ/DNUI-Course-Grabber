//! Interactive VPN sign-in in an isolated WebView2, never the user's browser profile.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::network::{SessionCookie, XK_LOGIN, XK_ORIGIN};

const CHANNEL_HELLO: &str = "DNUI_VPN_AUTH_V1\n";
const MAX_REPLY: u64 = 128 * 1024;

#[derive(Serialize, Deserialize)]
struct AuthReply {
    cookies: Option<Vec<SessionCookie>>,
    error: Option<String>,
}

pub fn is_xk_page(address: &str) -> bool {
    reqwest::Url::parse(address).is_ok_and(|url| {
        url.origin().ascii_serialization() == XK_ORIGIN && url.path().starts_with("/xsxk/")
    })
}

fn allowed_navigation(address: &str) -> bool {
    reqwest::Url::parse(address).is_ok_and(|url| {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.host_str().is_some_and(|host| {
                host == "neusoft.edu.cn"
                    || host.ends_with(".neusoft.edu.cn")
                    || host == "neuedu.com"
                    || host.ends_with(".neuedu.com")
            })
    })
}

fn decode_reply(bytes: &[u8]) -> Result<Vec<SessionCookie>> {
    if bytes.len() as u64 > MAX_REPLY {
        bail!("授权窗口返回数据过大");
    }
    let reply: AuthReply = serde_json::from_slice(bytes).map_err(|_| {
        anyhow::anyhow!("授权窗口未返回有效结果，请确认已安装 Microsoft Edge WebView2 Runtime")
    })?;
    if let Some(cookies) = reply.cookies {
        crate::network::session_jar(&cookies)?;
        Ok(cookies)
    } else {
        bail!(
            "{}",
            reply.error.unwrap_or_else(|| "VPN 授权已取消".to_owned())
        )
    }
}

#[cfg(windows)]
pub fn authorize() -> Result<Vec<SessionCookie>> {
    use std::{
        io::{Read, Write},
        os::windows::process::CommandExt,
        process::{Command, Stdio},
    };

    // Anonymous pipes keep session secrets out of command lines, files and logs.
    let mut child = Command::new(std::env::current_exe().context("无法定位授权程序")?)
        .arg("--vpn-auth-helper")
        .creation_flags(0x08000000) // No console; the interactive WebView remains visible.
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("无法启动 VPN 授权窗口")?;
    let result = (|| {
        child
            .stdin
            .take()
            .context("无法打开授权通道")?
            .write_all(CHANNEL_HELLO.as_bytes())
            .context("无法初始化授权通道")?;
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .context("无法读取授权通道")?
            .take(MAX_REPLY + 1)
            .read_to_end(&mut bytes)
            .context("读取授权结果失败")?;
        decode_reply(&bytes)
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    result
}

#[cfg(not(windows))]
pub fn authorize() -> Result<Vec<SessionCookie>> {
    bail!("当前平台暂不支持内置 VPN 授权窗口，请使用校园网或系统 VPN 隧道")
}

pub fn run_helper() {
    use std::io::{Read, Write};
    let mut hello = vec![0; CHANNEL_HELLO.len()];
    if std::io::stdin().read_exact(&mut hello).is_err() || hello != CHANNEL_HELLO.as_bytes() {
        return;
    }
    #[cfg(windows)]
    let result = window::run();
    #[cfg(not(windows))]
    let result = authorize();
    let reply = match result {
        Ok(cookies) => AuthReply {
            cookies: Some(cookies),
            error: None,
        },
        Err(error) => AuthReply {
            cookies: None,
            error: Some(error.to_string()),
        },
    };
    if let Ok(bytes) = serde_json::to_vec(&reply) {
        let _ = std::io::stdout().write_all(&bytes);
    }
}

#[cfg(windows)]
mod window {
    use super::*;
    use std::collections::BTreeMap;
    use winit::{
        application::ApplicationHandler,
        dpi::LogicalSize,
        event::WindowEvent,
        event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
        window::{Window, WindowId},
    };
    use wry::{
        NewWindowResponse, PageLoadEvent, WebContext, WebView, WebViewBuilder,
        WebViewBuilderExtWindows,
    };

    enum AuthEvent {
        Export,
        Navigate(String),
        Loaded(String),
    }

    struct AuthWindow {
        view: Option<WebView>,
        window: Option<Window>,
        proxy: EventLoopProxy<AuthEvent>,
        result: Option<Result<Vec<SessionCookie>>>,
    }

    impl ApplicationHandler<AuthEvent> for AuthWindow {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let result = (|| {
                let window = event_loop
                    .create_window(
                        Window::default_attributes()
                            .with_title(
                                "VPN 授权：请完成学校登录，返回选课页后点击“完成授权并检测”",
                            )
                            .with_inner_size(LogicalSize::new(1100.0, 800.0)),
                    )
                    .map_err(|_| anyhow::anyhow!("无法创建 VPN 授权窗口"))?;
                let ipc_proxy = self.proxy.clone();
                let nav_proxy = self.proxy.clone();
                let load_proxy = self.proxy.clone();
                let profile = std::env::var_os("LOCALAPPDATA")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(std::env::temp_dir)
                    .join("DNUI-XK")
                    .join("vpn-webview");
                let mut context = WebContext::new(Some(profile));
                let view = WebViewBuilder::new_with_web_context(&mut context)
                    // Keep the browser's security defaults (including SmartScreen).
                    .with_additional_browser_args("")
                    .with_incognito(true)
                    .with_url(XK_LOGIN)
                    .with_initialization_script(include_str!("vpn_auth.js"))
                    .with_navigation_handler(|url| allowed_navigation(&url))
                    .with_new_window_req_handler(move |url, _| {
                        if allowed_navigation(&url) {
                            let _ = nav_proxy.send_event(AuthEvent::Navigate(url));
                        }
                        NewWindowResponse::Deny
                    })
                    .with_on_page_load_handler(move |event, url| {
                        if matches!(event, PageLoadEvent::Finished) {
                            let _ = load_proxy.send_event(AuthEvent::Loaded(url));
                        }
                    })
                    .with_ipc_handler(move |request| {
                        if request.body() == "dnui-complete-vpn-auth"
                            && is_xk_page(&request.uri().to_string())
                        {
                            let _ = ipc_proxy.send_event(AuthEvent::Export);
                        }
                    })
                    .with_download_started_handler(|_, _| false)
                    .build(&window)
                    .map_err(|_| {
                        anyhow::anyhow!(
                            "无法初始化授权浏览器，请安装或修复 Microsoft Edge WebView2 Runtime"
                        )
                    })?;
                self.window = Some(window);
                self.view = Some(view);
                Ok(())
            })();
            if let Err(error) = result {
                self.result = Some(Err(error));
                event_loop.exit();
            }
        }

        fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AuthEvent) {
            let Some(view) = &self.view else {
                return;
            };
            match event {
                AuthEvent::Navigate(url) => {
                    let _ = view.load_url(&url);
                }
                AuthEvent::Loaded(url) => {
                    if let (Some(window), Ok(url)) = (&self.window, reqwest::Url::parse(&url)) {
                        // Do not put signed gateway query parameters in the title.
                        window.set_title(&format!(
                            "VPN 授权 — {} — 完成后点击页面底部按钮",
                            url.origin().ascii_serialization()
                        ));
                    }
                }
                AuthEvent::Export => {
                    if !view.url().is_ok_and(|url| is_xk_page(&url)) {
                        return;
                    }
                    let result = (|| {
                        let mut cookies = BTreeMap::new();
                        // HttpOnly/path-scoped cookies are read through the browser's
                        // supported API, only for the XK resources used by this app.
                        for path in [
                            "/xsxk/profile/index.html",
                            "/xsxk/auth/captcha",
                            "/xsxk/auth/login",
                            "/xsxk/elective/user",
                            "/xsxk/elective/grablessons",
                            "/xsxk/elective/clazz/list",
                            "/xsxk/elective/clazz/add",
                            "/xsxk/elective/clazz/del",
                            "/xsxk/web/now",
                        ] {
                            for cookie in view
                                .cookies_for_url(&format!("{XK_ORIGIN}{path}"))
                                .map_err(|_| anyhow::anyhow!("读取选课站点授权会话失败"))?
                            {
                                let entry = SessionCookie {
                                    name: cookie.name().to_owned(),
                                    value: cookie.value().to_owned(),
                                    path: cookie.path().unwrap_or("/").to_owned(),
                                };
                                cookies.insert((entry.name.clone(), entry.path.clone()), entry);
                            }
                        }
                        Ok(cookies.into_values().collect())
                    })();
                    self.result = Some(result);
                    event_loop.exit();
                }
            }
        }

        fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
            if event == WindowEvent::CloseRequested {
                event_loop.exit();
            }
        }
    }

    pub(super) fn run() -> Result<Vec<SessionCookie>> {
        let event_loop = EventLoop::<AuthEvent>::with_user_event()
            .build()
            .map_err(|_| anyhow::anyhow!("无法启动授权窗口消息循环"))?;
        let mut app = AuthWindow {
            window: None,
            view: None,
            proxy: event_loop.create_proxy(),
            result: None,
        };
        event_loop
            .run_app(&mut app)
            .map_err(|_| anyhow::anyhow!("授权窗口异常退出"))?;
        app.result.unwrap_or_else(|| {
            Err(anyhow::anyhow!(
                "VPN 授权已取消；请在授权窗口完成认证后点击“完成授权并检测”"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_school_https_navigation_is_allowed() {
        assert!(allowed_navigation(XK_LOGIN));
        assert!(allowed_navigation(
            "https://vpn.neuedu.com:4443/controller/v1/public/verify"
        ));
        assert!(allowed_navigation("https://cas.neusoft.edu.cn/login"));
        for url in [
            "http://xk.neusoft.edu.cn/",
            "https://vpn.neuedu.com.evil.test/",
            "file:///C:/data",
            "https://user@xk.neusoft.edu.cn/",
        ] {
            assert!(!allowed_navigation(url));
        }
    }

    #[test]
    fn ipc_cannot_export_from_vpn_or_foreign_origin() {
        assert!(is_xk_page(XK_LOGIN));
        for url in [
            "https://vpn.neuedu.com/xsxk/",
            "https://xk.neusoft.edu.cn:4443/xsxk/",
            "http://xk.neusoft.edu.cn/xsxk/",
            "https://xk.neusoft.edu.cn.evil.test/xsxk/",
        ] {
            assert!(!is_xk_page(url));
        }
    }

    #[test]
    fn invalid_reply_does_not_echo_secrets() {
        let message = decode_reply(b"TEST_SECRET invalid json")
            .err()
            .unwrap()
            .to_string();
        assert!(!message.contains("TEST_SECRET"));
        assert!(decode_reply(&vec![b'x'; MAX_REPLY as usize + 1]).is_err());
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires Windows WebView2; creates an invisible offline test window"]
    fn webview2_isolated_cookie_smoke_test() {
        use winit::{
            event_loop::EventLoop, platform::windows::EventLoopBuilderExtWindows, window::Window,
        };
        use wry::{WebContext, WebViewBuilder, WebViewBuilderExtWindows};
        let event_loop = EventLoop::builder().with_any_thread(true).build().unwrap();
        #[allow(deprecated)]
        let window = event_loop
            .create_window(Window::default_attributes().with_visible(false))
            .unwrap();
        let profile = std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("DNUI-XK")
            .join("vpn-webview-test");
        let mut context = WebContext::new(Some(profile));
        let view = WebViewBuilder::new_with_web_context(&mut context)
            .with_incognito(true)
            .with_additional_browser_args("")
            .with_html("<html><body>Offline WebView2 test</body></html>")
            .build(&window)
            .unwrap();
        let cookie = wry::cookie::Cookie::build(("dnui_smoke_test", "not-a-real-session"))
            .domain("xk.neusoft.edu.cn")
            .path("/")
            .secure(true)
            .http_only(true)
            .build();
        view.set_cookie(&cookie).unwrap();
        let cookies = view.cookies_for_url(XK_LOGIN).unwrap();
        assert!(
            cookies
                .iter()
                .any(|c| c.name() == "dnui_smoke_test" && c.value() == "not-a-real-session")
        );
        assert!(
            view.cookies_for_url("https://vpn.neuedu.com/")
                .unwrap()
                .is_empty()
        );
        for stored in &cookies {
            view.delete_cookie(stored).unwrap();
        }
        // WebView2's deletion is queued in its browser process; wait for the
        // change to become observable rather than assuming a synchronous API.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if view.cookies_for_url(XK_LOGIN).unwrap().is_empty() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "WebView2 did not finish deleting the test cookie"
            );
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
}
