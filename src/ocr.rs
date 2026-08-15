use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};

pub fn decode_data_url(data_url: &str) -> Result<Vec<u8>> {
    let payload = data_url
        .split_once(',')
        .map(|(_, payload)| payload)
        .unwrap_or(data_url);
    STANDARD
        .decode(payload)
        .context("验证码图片不是有效的 Base64")
}

pub fn recognize_captcha(data_url: &str) -> Result<String> {
    let bytes = decode_data_url(data_url)?;
    let mut ocr = ddddocr::ddddocr_classification().context("Rust ddddocr 初始化失败")?;
    ocr.set_ranges(6);
    let raw = ocr.classification(bytes).context("Rust ddddocr 识别失败")?;
    let answer: String = raw
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect();
    if answer.is_empty() {
        bail!("Rust ddddocr 未识别出验证码")
    }
    Ok(answer.chars().take(8).collect())
}

#[cfg(test)]
mod tests {
    use super::recognize_captcha;
    use serde_json::Value;

    #[test]
    #[ignore = "requires the live XK captcha service"]
    fn rust_ddddocr_recognizes_live_site_captcha() {
        let body: Value = reqwest::blocking::Client::new()
            .post("https://xk.neusoft.edu.cn/xsxk/auth/captcha")
            .send()
            .unwrap()
            .json()
            .unwrap();
        let image = body
            .pointer("/data/captcha")
            .and_then(Value::as_str)
            .unwrap();
        let answer = recognize_captcha(image).unwrap();
        // OCR can occasionally omit a noisy character; the login layer handles
        // that by requesting a fresh captcha and retrying.
        assert!(
            (1..=8).contains(&answer.len()),
            "unexpected OCR output: {answer:?}"
        );
        assert!(answer.chars().all(|ch| ch.is_ascii_alphanumeric()));
    }
}
