# Windows 高刷低占用鼠标特效悬浮窗开发记录 (AGENTS.md)

> [!IMPORTANT]
> **文档维护规范**：本文件作为项目的核心架构开发记录与避坑指南。当后续项目发生重大架构调整、底层 API 变更或新功能迭代时，**请在必要的时候同步更新本 `AGENTS.md` 文档**。

本项目是一个运行于 Windows 系统之上的高性能、低占用、高刷新率的透明悬浮窗程序，专为鼠标动态特效（如拖尾、点击波纹）以及光标本地强制重绘渲染（解决远程控制光标隐藏问题）设计。

项目完全采用 **Rust** 语言开发，基于 **Win32 API**、**Direct3D 11** 和 **DirectComposition** 图形合成技术，并在 Linux/WSL 环境下进行交叉编译。

---

## 1. 项目简介与功能特性

*   **项目路径**：`/home/nullsky/Workspaces/learn/cursor_overlay`
*   **彩虹渐变拖影 (Cursor Trails)**：利用 `VecDeque` 跟踪鼠标轨迹历史，采用 D3D11 渲染透明度递减、大小渐收的彩虹色圆点粒子。
*   **全局点击波纹 (Click Ripple)**：注册 Windows 全局低级鼠标钩子（`WH_MOUSE_LL`），在点击任意 Windows 程序时，于点击位置触发向外扩散渐隐的水波纹动画。
*   **光标强行渲染 (Remote Cursor Redraw)**：每帧查询 `HCURSOR` 状态，读取 GDI 光标位图（支持彩色和黑白双色光标），将其动态转化为 D3D11 纹理并缓存，精准按 Hotspot（热点）偏移对齐渲染，彻底解决远程桌面不显示光标的问题。
*   **多显示器自适应**：自动获取 Windows 虚拟桌面边界，将窗口铺满所有显示屏，并支持跨屏坐标计算与平移。

---

## 2. 核心技术栈选择

1.  **Rust + `windows` Crate**：
    使用微软官方维护的 `windows` 库，为 Rust 提供了零开销的 Win32/D3D11 绑定，既有 C++ 的极致性能，又具备 Rust 的内存安全性，没有垃圾回收（No GC）的卡顿。
2.  **D3D11 与 HLSL 着色器**：
    所有粒子和光标渲染完全在 GPU 执行，利用像素着色器（Pixel Shader）实现透明度混合与 HSL 彩虹颜色计算，运行极其流畅。
3.  **DirectComposition (DComp)**：
    Windows 8 引入的现代桌面合成技术。相比于传统的 GDI 透明或 `UpdateLayeredWindow`，它能将 D3D11 SwapChain 与桌面管理器（DWM）直接挂接，实现真正的硬件加速透明混合，CPU 占用率近乎为 0%。

---

## 3. 踩坑与错误经验记录 (Troubleshooting)

在项目从零搭建到顺利运行的过程中，我们经历了 5 个极具代表性的 DirectX 与 Windows 底层 API 严重“踩坑”，以下是详细的经验总结：

### 坑 1：`DXGI_ERROR_INVALID_CALL` (0x887A0001) —— SwapChain 尺寸参数无效
*   **现象**：程序在初始化 DXGI 交换链 `CreateSwapChainForComposition` 时崩溃，报无效调用错误。
*   **原因**：
    在普通的窗口化 SwapChain 中，我们将 `Width` 和 `Height` 传入 `0`，DXGI 会自动根据绑定的窗口句柄（`HWND`）查询并设置其大小。
    但 **DirectComposition 交换链并没有绑定的窗口句柄**（它是解耦的独立 Visual），DXGI 无法推断其尺寸。传入 `0` 会被判定为非法调用。
*   **解决**：在创建 SwapChain 前，必须调用 `GetSystemMetrics` 显式获取屏幕的宽高，并填入 `DXGI_SWAP_CHAIN_DESC1`。

---

### 坑 2：`E_INVALIDARG` (0x80070057) —— 常量缓冲区 (Constant Buffer) 对齐问题
*   **现象**：创建 Constant Buffer（`device.CreateBuffer`）时报参数无效错误。
*   **原因**：
    DirectX 11 对常量缓冲区的字节大小有极其严格的对齐要求：**`ByteWidth` 必须是 16 字节的整数倍**。
    原先我们的 Rust 结构体 `ConstantBufferData` 包含：`rect`（16 字节）+ `screen_size`（8 字节）+ `color`（16 字节） = 40 字节，40 字节不符合对齐要求。
    同时，HLSL 编译器在编译着色器时，由于变量对齐规则（`color` 作为 `float4` 必须重新对齐到 16 字节段起点），会在内部隐式插入 8 字节的 Padding。这导致 Rust 提交的 40 字节数据与 HLSL 预期的 48 字节数据发生冲突。
*   **解决**：在 Rust 结构体和 HLSL 的 cbuffer 定义中均显式添加 `padding: [f32; 2]` / `float2 padding`（8 字节），将大小刚好补足为 **48 字节**（16 字节的倍数）。

---

### 坑 3：`Present` 之后画面空无一物 —— 管道绑定的 RTV 每帧自动解绑
*   **现象**：初始化全无报错，程序成功进入循环，但桌面上看不到任何绘制的粒子与线条，窗口一片透明。
*   **原因**：
    在 DXGI 现代 Flip 渲染模型（`DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL`）下，**每次调用 `swap_chain.Present(...)` 呈现完画面后，DXGI 会强制把 Render Target View（RTV）从 Output Merger (OM) 渲染管道中解绑**。
    如果仅在初始化时绑定了一次 `OMSetRenderTargets`，那么从第二帧开始，所有的 GPU 绘制都是在“无 Render Target”的状态下执行的，数据会被全部丢弃。
*   **解决**：必须在主渲染循环的每一帧，在清屏 `ClearRenderTargetView` 执行前，**重新调用 `OMSetRenderTargets` 重新绑定 RTV**。

---

### 坑 4：多屏幕环境下副屏幕完全黑屏 —— 传统分层属性与 DComp 混色冲突
*   **现象**：启动软件后，主屏一切正常，但副显示屏突然变成纯黑色，桌面上所有原本打开的窗口都被遮挡。
*   **原因**：
    我们在 `create_window` 时，调用了传统的 Win32 API：
    `SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)`。
    这行代码在系统底层将分层窗口的透明度写死为了 `255`（完全不透明）。对于主显示器，DirectComposition 驱动在合成时强行覆盖并实现了透明；但在多屏幕下，副屏幕的 DWM 合成器并没有正确覆盖该设置，而是将其视为了一个“完全不透明的纯黑大窗口”遮盖了整个屏幕，导致黑屏。
*   **解决**：**彻底删掉或注释 `SetLayeredWindowAttributes`**。对于 DirectComposition，只需窗口带 `WS_EX_LAYERED` 属性，DirectComposition 和 SwapChain 的 `DXGI_ALPHA_MODE_PREMULTIPLIED` 会自动以最高性能处理系统级别的透明混合，绝不应去调用传统的 Layered 属性设置函数。

---

### 坑 5：多屏幕下鼠标在副屏越界和窗口覆盖不全
*   **现象**：副屏幕上不仅黑屏，而且即便解决了黑屏，鼠标移动到副屏后，拖尾粒子也看不见。
*   **原因**：
    *   单屏 API `SM_CXSCREEN`/`SM_CYSCREEN` 只会返回主显示器的尺寸，导致我们的悬浮窗只建立在主屏幕上。
    *   在 Windows 多屏幕体系中，所有屏幕组合成一个巨大的“虚拟桌面”（Virtual Desktop）。副屏幕的物理坐标原点通常与主屏幕不同，若副屏幕排在主屏左侧或上方，其坐标 `ptScreenPos` 甚至会出现**负数**（例如 `-1920, 0`）。
    *   由于我们的 D3D11 视口只限于主屏大小，一旦鼠标去往副屏，其坐标在着色器映射为 NDC 坐标时会超出 $[-1.0, 1.0]$ 的边界，被 GPU 直接裁剪。
*   **解决**：
    *   使用虚拟屏幕宏 `SM_XVIRTUALSCREEN`, `SM_YVIRTUALSCREEN`, `SM_CXVIRTUALSCREEN`, `SM_CYVIRTUALSCREEN` 初始化窗口坐标和 SwapChain 尺寸，使窗口横跨铺满所有显示屏。
    *   在渲染主循环和鼠标 Hook 接收到鼠标坐标后，**减去虚拟屏幕的起点偏移（`screen_x` / `screen_y`）** 进行本地化重映射，确保坐标映射进整个虚拟屏幕视口内。

### 坑 6：任务栏遮挡悬浮窗（无法覆盖任务栏置顶）
*   **现象**：程序启动后，拖尾和点击波纹可以在其他软件窗口上层绘制，但一旦鼠标移动到任务栏区域，特效会被 Windows 任务栏盖住。
*   **原因**：
    Windows 桌面管理器的 Z-Order 是分层级的。任务栏（Shell_TrayWnd）以及开始菜单通常位于更高级别的系统级 Z-Band，而我们通过 `WS_EX_TOPMOST` 属性创建的悬浮窗由于设置了 `WS_EX_NOACTIVATE` 且从不被激活，在用户与任务栏交互时，会被 DWM 排序到任务栏的下方。
*   **解决**：
    在主渲染循环中增加一个轻量级的帧计数器，**每隔 120 帧（高刷屏下约 1 秒左右）**，调用一次 `SetWindowPos` 将窗口强制重设为 `HWND_TOPMOST`，并传入 `SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE` 标识。这样可以在不抢占用户焦点、不造成画面闪烁的前提下，保证悬浮窗持续处于绝对顶层（包括覆盖任务栏）。
### 坑 7：强制光标重绘时背景出现不透明白色方块
*   **现象**：当在屏幕上渲染光标且底下有其他特效（如点击水波纹）时，光标周围会产生一个纯白色的不透明正方形色块，把下方的背景画面完全挡住。
*   **原因**：
    部分 Windows 彩色光标的 `hbmColor` 虽然是 32 位位图，但它的 Alpha 通道在 GDI 中可能全部为 0（不包含预乘透明度）。先前我们的处理方式是，若检查到全 0 Alpha，就粗暴地把整个位图所有像素的 Alpha 全部设为 255（不透明），这导致光标周围原本应该透明的白色背景区域变成了纯白色的不透明色块。
*   **解决**：
    在 Windows GDI 体系中，即使是彩色光标，其透明度信息也常常存储在伴随的 `hbmMask`（AND 掩码）中。我们重构了 `cursor_to_rgba` 函数，同时读取 `hbmColor` 和 `hbmMask`。对于每一像素进行遍历：若 AND 掩码对应的像素为 1（白色），则将其判定为透明色，直接强制将其 RGBA 像素重设为完全透明（`0x00000000`）；若掩码对应的像素为 0，且原色彩像素 Alpha 为 0，再将其设为不透明（`0xFF000000`）。通过结合 Mask 掩码与 Color 颜色信息，彻底修复了光标周围出现不透明白色方块的渲染 bug。

---

## 4. WSL 交叉编译与运行指南

### 依赖准备
在 WSL 终端中安装链接器和 Rust 编译目标：
```bash
sudo apt update && sudo apt install -y mingw-w64
rustup target add x86_64-pc-windows-gnu
```

### 编译
在项目根目录下执行：
```bash
cargo build --target x86_64-pc-windows-gnu
```

### 运行
得益于 WSL 的 Windows Interoperability 机制，可直接在 WSL 或 Windows PowerShell 中通过相对路径运行产生的 `.exe`。
在 Windows 终端中运行：
```powershell
cd /home/nullsky/Workspaces/learn/cursor_overlay
.\target\x86_64-pc-windows-gnu\debug\cursor_overlay.exe
```
按下 `Ctrl + C` 即可完美关闭退出。

---

## 5. GitHub Actions 自动构建与发布机制 (CI/CD)

项目在 `.github/workflows/release.yml` 中配置了自动化 CI/CD 工作流，以便在发布新版本时自动编译并发布 Windows 原生 `.exe` 二进制文件。

### 触发机制
当向 GitHub 仓库推送以 `v` 开头的标签（例如 `v1.0.0`）时，工作流会自动触发。

### 构建环境与流程
1. **构建环境**：运行在微软官方的 `windows-latest` 容器中。这允许我们直接使用原生 Windows SDK 和链接器，避免了交叉编译的链接复杂性。
2. **编译器目标**：使用 `x86_64-pc-windows-msvc` 目标。编译产物相比于 MinGW-w64 (`x86_64-pc-windows-gnu`) 更加原生，没有任何第三方运行时库依赖。
3. **缓存优化**：利用 `Swatinem/rust-cache` 对 Cargo 依赖进行缓存，加快后续构建速度。
4. **发布附件**：编译完成后，自动在 GitHub 上创建一个新的 Release，并将编译好的免安装绿色版 `cursor_overlay.exe` 上传为发布附件。

### 使用方法
在本地代码开发完成后，只需打上 Tag 并推送即可触发自动发布：
```bash
# 1. 在本地打上版本标签
git tag v1.0.0

# 2. 推送标签到 GitHub (确保开启代理)
export http_proxy=http://127.0.0.1:7897 && export https_proxy=http://127.0.0.1:7897
git push origin v1.0.0
```
