# 选课工具

现有 Python/Tkinter 版本的独立 Rust 重构。功能包括：

- Rust 原生 ddddocr 验证码识别（内嵌 ONNX 模型，不依赖 Python/OpenCV）
- 登录、普通/实验轮次选择
- 从所选轮次页面动态识别 `FAWKC` / `TJKC` / `XGKC` 等课程类型
- 分页抓取、搜索过滤、仅显示可选课程
- 多选课程、定时开始、重复抢课、保活、暂停与停止

## 构建

```powershell
cargo build --release
```

输出：`target\release\xk-rust.exe`

首次构建会下载 Rust ddddocr 依赖和 ONNX Runtime 静态库，因此耗时较长。

## 安全

偏好和课程缓存保存在 `%LOCALAPPDATA%\DNUI-XK\`，不会在可执行文件目录生成 JSON 文件。仓库不会复制 Python 版的账号、密码或课程缓存。
