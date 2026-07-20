# Sonora

Sonora 是一个使用 Rust 编写的跨平台桌面 MP3 播放器。它以 `egui/eframe` 提供原生桌面界面，使用 `rodio` 管理播放和设备、`symphonia` 探测 MP3 音频信息，并支持同步 LRC 歌词。

## 功能

- 播放、暂停、停止、上一曲、下一曲、拖动跳转
- 音量 0–100、静音、0.5×–2.0× 倍速
- 顺序、单曲循环、列表循环、随机四种播放模式
- 枚举并热切换系统音频输出设备，切换后恢复进度和播放状态
- 批量导入 MP3 或递归扫描文件夹
- 读取 ID3 标题、歌手、专辑和内嵌封面，缺失时从 `歌手 - 歌名.mp3` 推导
- 自动匹配同目录同名 `.lrc`，也可手动选择歌词
- 当前歌词高亮、自动居中，点击歌词跳转
- 播放列表双击播放、删除、清空，以及按住列表项拖放排序
- 自动保存音量、倍速、播放模式、主题和输出设备配置

## 构建

需要 Rust 1.85 或更高版本（项目使用 Rust 2024 edition）。

```powershell
cargo build --release
cargo run --release
```

生成的程序位于 Windows 的 `target/release/sonora.exe`，macOS/Linux 为 `target/release/sonora`。

Linux 需要系统提供 ALSA 开发库；Debian/Ubuntu 可安装：

```bash
sudo apt install libasound2-dev pkg-config
```

## 操作

1. 点击右侧“添加”选择一个或多个 MP3，或点击“文件夹”递归扫描目录。
2. 双击列表中的歌曲开始播放；同名 LRC 会自动加载。
3. 使用底栏控制播放、进度、模式、倍速和音量。
4. 点击歌词行可跳转；顶部“选择歌词”可覆盖当前歌曲的自动匹配结果。
5. 按住播放列表项并拖到目标项上释放，可调整顺序。

LRC 支持 `[mm:ss]`、`[mm:ss.xx]`、同一行多个时间标签，以及 `ti/ar/al/by` 元数据。无法识别的标签和损坏行会被跳过。

## 架构

- `src/main.rs`：原生窗口入口和模块装配。
- `src/player.rs`：独立音频线程。线程独占 rodio 输出流和播放器，UI 通过标准消息通道发送命令，并接收位置、完成、设备及错误事件。
- `src/lrc.rs`：无正则依赖的容错 LRC 解析器。以有序时间表保留重复时间戳，通过二分定位当前歌词。
- `src/playlist.rs`：ID3 元数据、封面、symphonia 时长探测、文件夹扫描和列表重排。
- `src/ui.rs`：egui 即时界面、状态协调、封面纹理和 JSON 配置持久化。
- `src/utils.rs`：时间显示、扩展名判断和文件名元数据兜底。

详细数据流和设计取舍见 [`docs/architecture.md`](docs/architecture.md)。

## 平台说明

音频设备由 CPAL（rodio 的底层）抽象，因此代码不包含 Windows、macOS 或 Linux 专属播放逻辑。中文字体从各平台常见系统字体中按顺序选择；若系统未安装候选字体，egui 会回退到内置字体。

