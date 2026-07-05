# Windows 高刷低占用鼠标特效悬浮窗 (Cursor Overlay)

一个基于 **Rust**、**Direct3D 11** 和 **DirectComposition** 构建的高性能、低占用、高刷新率的透明鼠标特效悬浮窗程序。

本项目专为追求极致流畅度的玩家及远程桌面用户开发，旨在提供高刷鼠标拖影、点击特效，并解决远程桌面连接时不显示远程光标的痛点。

![演示动画](assets/demo.gif)

---

## ✨ 功能特性

1.  **🌈 彩虹渐变鼠标拖影 (Cursor Trails)**
    *   实时捕捉鼠标移动轨迹，动态绘制渐缩、渐隐的彩虹色粒子拖尾。
    *   渲染完全交由 GPU 处理，无垃圾回收 (No GC) 导致的任何微小卡顿。
2.  **💧 全局点击波纹特效 (Click Ripples)**
    *   通过低级鼠标钩子 (`WH_MOUSE_LL`) 监听全局点击事件，支持在点击屏幕任意位置时触发轻量级的淡蓝色水波纹向外扩散动画。
3.  **🖱️ 本地光标强制重绘 (Cursor Redraw)**
    *   针对部分远程桌面软件（如向日葵、ToDesk、RDP 等）不显示远程光标的问题，程序会实时捕获并解码系统光标样式为 D3D11 纹理，并在当前坐标强制绘制，解决“找不到光标”的烦恼。
4.  **🖥️ 完美支持多显示器 (Multi-Monitor)**
    *   自动适配 Windows 虚拟桌面边界，将悬浮窗铺满所有屏幕，并在跨屏移动时自动换算坐标。
5.  **⚡ 极致流畅与低占用**
    *   基于 **DirectComposition** 技术，将 D3D11 SwapChain 与 Windows DWM 组合树挂接，实现硬件加速透明合成，CPU 占用率近乎 0%。
    *   解除任何休眠锁帧，自动利用 V-Sync 同步至你的显示器原生最高刷新率（如 144Hz / 240Hz / 360Hz+）。

---

## 🚀 快速开始

### 方式 A：从 WSL 交叉编译并运行（推荐）

本方案默认使用 Linux (WSL) 编译为 Windows 可执行文件并运行：

1.  **在 WSL 中准备 MinGW 工具链与 Rust 编译目标**：
    ```bash
    sudo apt update && sudo apt install -y mingw-w64
    rustup target add x86_64-pc-windows-gnu
    ```
2.  **编译项目**：
    ```bash
    cargo build --target x86_64-pc-windows-gnu
    ```
3.  **运行程序**：
    在 Windows PowerShell 或 cmd 中，进入项目的 debug 输出目录直接运行即可：
    ```powershell
    .\target\x86_64-pc-windows-gnu\debug\cursor_overlay.exe
    ```
    *在运行的终端中按 `Ctrl + C` 即可安全释放资源并退出悬浮窗。*

### 方式 B：在 Windows 本地编译

如果你在 Windows 宿主机上安装了 Rust 环境：
```powershell
# 编译 Release 版本
cargo build --release

# 运行
.\target\release\cursor_overlay.exe
```



## 🤝 参与开发

若想增加更丰富的鼠标样式（如星光粒子、着色器水彩特效等），可在 `src/main.rs` 的 D3D11 绘制逻辑或顶点/像素着色器中进行修改。欢迎提交 Issue 与 Pull Request！
