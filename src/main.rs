use std::collections::VecDeque;
use std::mem::size_of;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::{
    core::*,
    Win32::Foundation::*,
    Win32::UI::WindowsAndMessaging::*,
    Win32::System::LibraryLoader::GetModuleHandleW,
    Win32::Graphics::Direct3D::*,
    Win32::Graphics::Direct3D11::*,
    Win32::Graphics::Dxgi::*,
    Win32::Graphics::Dxgi::Common::*,
    Win32::Graphics::DirectComposition::*,
    Win32::Graphics::Gdi::*,
};

// =========================================================================
// Global Mouse Hook (for click events)
// =========================================================================
static CLICK_EVENTS: OnceLock<Mutex<Vec<(f32, f32)>>> = OnceLock::new();
static mut MOUSE_HOOK: HHOOK = HHOOK(std::ptr::null_mut());

unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let mouse_struct = *(lparam.0 as *const MSLLHOOKSTRUCT);
        match wparam.0 as u32 {
            WM_LBUTTONDOWN => {
                let x = mouse_struct.pt.x as f32;
                let y = mouse_struct.pt.y as f32;
                if let Some(mutex) = CLICK_EVENTS.get() {
                    if let Ok(mut guard) = mutex.lock() {
                        guard.push((x, y));
                    }
                }
            }
            _ => {}
        }
    }
    CallNextHookEx(MOUSE_HOOK, code, wparam, lparam)
}

fn start_mouse_hook() {
    unsafe {
        CLICK_EVENTS.set(Mutex::new(Vec::new())).ok();
        MOUSE_HOOK = SetWindowsHookExW(
            WH_MOUSE_LL,
            Some(mouse_hook_proc),
            None,
            0,
        ).unwrap();
    }
}

fn stop_mouse_hook() {
    unsafe {
        if !MOUSE_HOOK.is_invalid() {
            let _ = UnhookWindowsHookEx(MOUSE_HOOK);
        }
    }
}

// =========================================================================
// Data Structures for Effects
// =========================================================================
struct TrailPoint {
    x: f32,
    y: f32,
    time: Instant,
}

struct ClickRipple {
    x: f32,
    y: f32,
    start_time: Instant,
    max_radius: f32,
    duration: f32, // in seconds
}

#[repr(C)]
struct Vertex {
    pos: [f32; 2],
    tex: [f32; 2],
}

#[repr(C)]
struct ConstantBufferData {
    rect: [f32; 4],        // x, y, width, height (in screen pixels)
    screen_size: [f32; 2], // screen width, height
    padding: [f32; 2],     // padding to align to 16-byte boundary
    color: [f32; 4],       // RGBA multiplier
}

// =========================================================================
// GDI Cursor to D3D11 Texture Conversion
// =========================================================================
unsafe fn cursor_to_rgba(h_cursor: HCURSOR) -> Option<(Vec<u32>, u32, u32, i32, i32)> {
    let mut icon_info = ICONINFO::default();
    if GetIconInfo(h_cursor, &mut icon_info).is_err() {
        return None;
    }

    let hdc = CreateCompatibleDC(None);
    if hdc.is_invalid() {
        return None;
    }

    let h_bmp = if !icon_info.hbmColor.is_invalid() {
        icon_info.hbmColor
    } else {
        icon_info.hbmMask
    };

    let mut bmp = BITMAP::default();
    let obj_res = GetObjectW(
        HGDIOBJ(h_bmp.0),
        size_of::<BITMAP>() as i32,
        Some(&mut bmp as *mut _ as *mut _),
    );
    if obj_res == 0 {
        DeleteDC(hdc);
        return None;
    }

    let width = bmp.bmWidth as u32;
    let height = if !icon_info.hbmColor.is_invalid() {
        bmp.bmHeight as u32
    } else {
        (bmp.bmHeight / 2) as u32
    };

    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32), // Top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0, // BI_RGB
            ..Default::default()
        },
        ..Default::default()
    };

    let mut pixels = vec![0u32; (width * height) as usize];
    
    if !icon_info.hbmColor.is_invalid() {
        let res = GetDIBits(
            hdc,
            icon_info.hbmColor,
            0,
            height,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_USAGE(0), // DIB_RGB_COLORS
        );
        if res == 0 {
            DeleteDC(hdc);
            return None;
        }

        // Fix alpha channel: if all pixels have 0 alpha, make them opaque
        let mut has_alpha = false;
        for p in &pixels {
            if (*p >> 24) != 0 {
                has_alpha = true;
                break;
            }
        }
        if !has_alpha {
            for p in &mut pixels {
                *p |= 0xFF000000;
            }
        }
    } else {
        // Monochrome cursor: top half is AND mask, bottom half is XOR mask
        let double_height = height * 2;
        let mut mask_pixels = vec![0u32; (width * double_height) as usize];
        let mut bmi_mono = bmi;
        bmi_mono.bmiHeader.biHeight = -(double_height as i32);
        
        let res = GetDIBits(
            hdc,
            icon_info.hbmMask,
            0,
            double_height,
            Some(mask_pixels.as_mut_ptr() as *mut _),
            &mut bmi_mono,
            DIB_USAGE(0),
        );
        if res == 0 {
            DeleteDC(hdc);
            return None;
        }

        for y in 0..height {
            for x in 0..width {
                let idx_and = (y * width + x) as usize;
                let idx_xor = ((y + height) * width + x) as usize;
                
                let and_pixel = mask_pixels[idx_and];
                let xor_pixel = mask_pixels[idx_xor];
                
                let is_transparent = (and_pixel & 1) != 0;
                let is_white = (xor_pixel & 1) != 0;
                
                pixels[idx_and] = if is_transparent {
                    0x00000000
                } else if is_white {
                    0xFFFFFFFF
                } else {
                    0xFF000000
                };
            }
        }
    }

    if !icon_info.hbmColor.is_invalid() { let _ = DeleteObject(icon_info.hbmColor); }
    if !icon_info.hbmMask.is_invalid() { let _ = DeleteObject(icon_info.hbmMask); }
    let _ = DeleteDC(hdc);

    Some((pixels, width, height, icon_info.xHotspot as i32, icon_info.yHotspot as i32))
}

unsafe fn create_texture_from_rgba(
    device: &ID3D11Device,
    pixels: &[u32],
    width: u32,
    height: u32,
) -> Result<ID3D11ShaderResourceView> {
    let texture_desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_IMMUTABLE,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };

    let subresource_data = D3D11_SUBRESOURCE_DATA {
        pSysMem: pixels.as_ptr() as *const _,
        SysMemPitch: width * 4,
        SysMemSlicePitch: 0,
    };

    let mut texture = None;
    device.CreateTexture2D(&texture_desc, Some(&subresource_data), Some(&mut texture))?;
    let texture = texture.unwrap();

    let mut srv = None;
    device.CreateShaderResourceView(&texture, None, Some(&mut srv))?;
    Ok(srv.unwrap())
}

// Generate smooth white circle texture (RGBA)
fn create_circle_texture(device: &ID3D11Device) -> Result<ID3D11ShaderResourceView> {
    let size = 64;
    let mut pixels = vec![0u32; size * size];
    let center = size as f32 / 2.0;
    let radius = size as f32 / 2.0;
    
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let dist = (dx*dx + dy*dy).sqrt();
            
            if dist < radius {
                let alpha = ((radius - dist) / 1.5).clamp(0.0, 1.0);
                let alpha_byte = (alpha * 255.0) as u32;
                pixels[y * size + x] = (alpha_byte << 24) | 0x00FFFFFF; // White with antialiased alpha
            } else {
                pixels[y * size + x] = 0;
            }
        }
    }
    
    unsafe { create_texture_from_rgba(device, &pixels, size as u32, size as u32) }
}

// =========================================================================
// Shader Compiling
// =========================================================================
fn compile_shader(source: &str, entry_point: &str, target: &str) -> Result<ID3DBlob> {
    unsafe {
        let mut blob = None;
        let mut error_blob = None;
        
        let entry_point_c = std::ffi::CString::new(entry_point).unwrap();
        let target_c = std::ffi::CString::new(target).unwrap();
        
        let hr = windows::Win32::Graphics::Direct3D::Fxc::D3DCompile(
            source.as_ptr() as *const _,
            source.len(),
            None,
            None,
            None,
            PCSTR(entry_point_c.as_ptr() as *const u8),
            PCSTR(target_c.as_ptr() as *const u8),
            0,
            0,
            &mut blob,
            Some(&mut error_blob),
        );
        
        if hr.is_err() {
            if let Some(err) = error_blob {
                let error_msg = std::slice::from_raw_parts(
                    err.GetBufferPointer() as *const u8,
                    err.GetBufferSize(),
                );
                eprintln!("Shader compile error: {}", String::from_utf8_lossy(error_msg));
            }
        }
        hr?;
        
        Ok(blob.unwrap())
    }
}

// =========================================================================
// Window Creation
// =========================================================================
unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as _),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn create_window() -> Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class_name = wstring("CursorOverlayClass");

        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            ..Default::default()
        };

        RegisterClassW(&wc);

        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            PCWSTR(class_name.as_ptr()),
            w!("Cursor Overlay"),
            WS_POPUP | WS_VISIBLE,
            vx,
            vy,
            vw,
            vh,
            None,
            None,
            instance,
            None,
        )?;

        // NOTE: For DirectComposition windows, do NOT call SetLayeredWindowAttributes.
        // Doing so can conflict with DirectComposition and cause secondary monitors to go black.
        // SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;

        Ok(hwnd)
    }
}

fn wstring(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// =========================================================================
// Main Program
// =========================================================================
fn main() -> Result<()> {
    unsafe {
        // Make the process High-DPI aware
        let _ = SetProcessDPIAware();

        println!("Starting Windows Cursor Overlay from WSL...");
        
        // Start low-level mouse hook to capture click events
        start_mouse_hook();
        println!("Mouse hook started successfully.");

        let hwnd = create_window()?;
        println!("Window created successfully!");

        // D3D11 Device & Context
        println!("Creating D3D11 Device...");
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;

        let mut hr = D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        );

        if hr.is_err() {
            println!("Failed to create D3D11 hardware device (Error: {:?}), trying WARP software renderer...", hr);
            hr = D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_WARP,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            );
        }
        hr?;

        let device = device.unwrap();
        let context = context.unwrap();
        println!("D3D11 Device created successfully!");

        // DXGI SwapChain
        println!("Querying DXGI Factory...");
        let dxgi_device: IDXGIDevice = device.cast()?;
        let dxgi_adapter = dxgi_device.GetAdapter()?;
        let dxgi_factory: IDXGIFactory2 = dxgi_adapter.GetParent()?;

        let screen_width_init = GetSystemMetrics(SM_CXVIRTUALSCREEN) as u32;
        let screen_height_init = GetSystemMetrics(SM_CYVIRTUALSCREEN) as u32;

        let swap_chain_desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: screen_width_init,
            Height: screen_height_init,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: BOOL(0),
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: 0,
        };

        println!("Creating DXGI SwapChain for Composition...");
        let swap_chain = dxgi_factory.CreateSwapChainForComposition(&device, &swap_chain_desc, None)?;
        println!("DXGI SwapChain created successfully!");

        // DirectComposition
        println!("Initializing DirectComposition...");
        let dcomp_device: IDCompositionDevice = DCompositionCreateDevice(&dxgi_device)?;
        let dcomp_target = dcomp_device.CreateTargetForHwnd(hwnd, true)?;

        let visual = dcomp_device.CreateVisual()?;
        visual.SetContent(&swap_chain)?;
        dcomp_target.SetRoot(&visual)?;
        dcomp_device.Commit()?;
        println!("DirectComposition committed successfully!");

        // Render Target View
        println!("Creating D3D11 Render Target View...");
        let back_buffer: ID3D11Texture2D = swap_chain.GetBuffer(0)?;
        let mut render_target_view: Option<ID3D11RenderTargetView> = None;
        device.CreateRenderTargetView(&back_buffer, None, Some(&mut render_target_view))?;
        let render_target_view = render_target_view.unwrap();
        println!("D3D11 Render Target View created successfully!");

        // Blend State for transparency support
        let blend_desc = D3D11_BLEND_DESC {
            AlphaToCoverageEnable: BOOL(0),
            IndependentBlendEnable: BOOL(0),
            RenderTarget: [
                D3D11_RENDER_TARGET_BLEND_DESC {
                    BlendEnable: BOOL(1),
                    SrcBlend: D3D11_BLEND_SRC_ALPHA,
                    DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                    BlendOp: D3D11_BLEND_OP_ADD,
                    SrcBlendAlpha: D3D11_BLEND_ONE,
                    DestBlendAlpha: D3D11_BLEND_ZERO,
                    BlendOpAlpha: D3D11_BLEND_OP_ADD,
                    RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
                },
                Default::default(), Default::default(), Default::default(),
                Default::default(), Default::default(), Default::default(), Default::default(),
            ],
        };
        let mut blend_state = None;
        device.CreateBlendState(&blend_desc, Some(&mut blend_state))?;
        let blend_state = blend_state.unwrap();

        // HLSL Shaders
        let shader_code = r#"
        struct VS_INPUT {
            float2 pos : POSITION;
            float2 tex : TEXCOORD;
        };
        struct PS_INPUT {
            float4 pos : SV_POSITION;
            float2 tex : TEXCOORD;
        };
        cbuffer ConstantBuffer : register(b0) {
            float4 rect;        // x, y, width, height (in screen pixels)
            float2 screenSize;  // screen width, height
            float2 padding;     // 8 bytes padding
            float4 color;       // RGBA multiplier
        };
        PS_INPUT vs_main(VS_INPUT input) {
            PS_INPUT output;
            float2 screenPos = rect.xy + input.pos * rect.zw;
            output.pos.x = (screenPos.x / screenSize.x) * 2.0f - 1.0f;
            output.pos.y = 1.0f - (screenPos.y / screenSize.y) * 2.0f;
            output.pos.z = 0.0f;
            output.pos.w = 1.0f;
            output.tex = input.tex;
            return output;
        }
        Texture2D shaderTexture : register(t0);
        SamplerState sampleState : register(s0);
        float4 ps_main(PS_INPUT input) : SV_TARGET {
            return shaderTexture.Sample(sampleState, input.tex) * color;
        }
        "#;

        let vs_blob = compile_shader(shader_code, "vs_main", "vs_4_0")?;
        let ps_blob = compile_shader(shader_code, "ps_main", "ps_4_0")?;

        let mut vertex_shader = None;
        device.CreateVertexShader(
            std::slice::from_raw_parts(vs_blob.GetBufferPointer() as *const u8, vs_blob.GetBufferSize()),
            None,
            Some(&mut vertex_shader),
        )?;
        let vertex_shader = vertex_shader.unwrap();

        let mut pixel_shader = None;
        device.CreatePixelShader(
            std::slice::from_raw_parts(ps_blob.GetBufferPointer() as *const u8, ps_blob.GetBufferSize()),
            None,
            Some(&mut pixel_shader),
        )?;
        let pixel_shader = pixel_shader.unwrap();

        // Input Layout
        let layout_desc = [
            D3D11_INPUT_ELEMENT_DESC {
                SemanticName: PCSTR(b"POSITION\0".as_ptr()),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: 0,
                InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
            D3D11_INPUT_ELEMENT_DESC {
                SemanticName: PCSTR(b"TEXCOORD\0".as_ptr()),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: D3D11_APPEND_ALIGNED_ELEMENT,
                InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
        ];

        let mut input_layout = None;
        device.CreateInputLayout(
            &layout_desc,
            std::slice::from_raw_parts(vs_blob.GetBufferPointer() as *const u8, vs_blob.GetBufferSize()),
            Some(&mut input_layout),
        )?;
        let input_layout = input_layout.unwrap();

        // Quad Vertices (Triangle Strip)
        let vertices = [
            Vertex { pos: [0.0, 0.0], tex: [0.0, 0.0] },
            Vertex { pos: [1.0, 0.0], tex: [1.0, 0.0] },
            Vertex { pos: [0.0, 1.0], tex: [0.0, 1.0] },
            Vertex { pos: [1.0, 1.0], tex: [1.0, 1.0] },
        ];

        let vertex_buffer_desc = D3D11_BUFFER_DESC {
            ByteWidth: size_of::<[Vertex; 4]>() as u32,
            Usage: D3D11_USAGE_IMMUTABLE,
            BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
            ..Default::default()
        };

        let vertex_data = D3D11_SUBRESOURCE_DATA {
            pSysMem: vertices.as_ptr() as *const _,
            ..Default::default()
        };

        let mut vertex_buffer = None;
        device.CreateBuffer(&vertex_buffer_desc, Some(&vertex_data), Some(&mut vertex_buffer))?;
        let vertex_buffer = vertex_buffer.unwrap();

        // Constant Buffer
        let cb_desc = D3D11_BUFFER_DESC {
            ByteWidth: size_of::<ConstantBufferData>() as u32,
            Usage: D3D11_USAGE_DYNAMIC,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            ..Default::default()
        };
        let mut constant_buffer = None;
        device.CreateBuffer(&cb_desc, None, Some(&mut constant_buffer))?;
        let constant_buffer = constant_buffer.unwrap();

        // Sampler State
        let sampler_desc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            ComparisonFunc: D3D11_COMPARISON_NEVER,
            MinLOD: 0.0,
            MaxLOD: D3D11_FLOAT32_MAX,
            ..Default::default()
        };
        let mut sampler_state = None;
        device.CreateSamplerState(&sampler_desc, Some(&mut sampler_state))?;
        let sampler_state = sampler_state.unwrap();

        // Circular shape texture for trailing and click waves
        let circle_srv = create_circle_texture(&device)?;

        // Cursor Texture Cache: map hCursor values to (SRV, width, height, hotspot_x, hotspot_y)
        let mut cursor_cache: std::collections::HashMap<isize, (ID3D11ShaderResourceView, u32, u32, i32, i32)> = std::collections::HashMap::new();

        // State variables
        let mut trail: VecDeque<TrailPoint> = VecDeque::new();
        let mut click_ripples: Vec<ClickRipple> = Vec::new();

        println!("Direct3D 11 & DirectComposition successfully loaded!");
        println!("Render loop started. Press Ctrl+C in WSL terminal to exit.");

        let mut msg = MSG::default();
        loop {
            // Process window messages (needed for hotkeys, resizing, DComp synchronization)
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
                if msg.message == WM_QUIT {
                    stop_mouse_hook();
                    return Ok(());
                }
            }

            // Get current screen resolution (virtual screen)
            let screen_x = GetSystemMetrics(SM_XVIRTUALSCREEN) as f32;
            let screen_y = GetSystemMetrics(SM_YVIRTUALSCREEN) as f32;
            let screen_width = GetSystemMetrics(SM_CXVIRTUALSCREEN) as f32;
            let screen_height = GetSystemMetrics(SM_CYVIRTUALSCREEN) as f32;

            // Get current cursor information
            let mut cursor_info = CURSORINFO {
                cbSize: size_of::<CURSORINFO>() as u32,
                ..Default::default()
            };
            let mut current_mouse_x = 0.0;
            let mut current_mouse_y = 0.0;
            let mut has_cursor = false;
            let mut cursor_srv: Option<ID3D11ShaderResourceView> = None;
            let mut cursor_w = 32u32;
            let mut cursor_h = 32u32;
            let mut hotspot_x = 0i32;
            let mut hotspot_y = 0i32;

            if GetCursorInfo(&mut cursor_info).is_ok() {
                current_mouse_x = cursor_info.ptScreenPos.x as f32 - screen_x;
                current_mouse_y = cursor_info.ptScreenPos.y as f32 - screen_y;
                
                // Read and cache cursor texture
                let h_cursor = cursor_info.hCursor;
                if !h_cursor.is_invalid() {
                    let cache_key = h_cursor.0 as isize;
                    if !cursor_cache.contains_key(&cache_key) {
                        if let Some((pixels, w, h, hx, hy)) = cursor_to_rgba(h_cursor) {
                            if let Ok(srv) = create_texture_from_rgba(&device, &pixels, w, h) {
                                cursor_cache.insert(cache_key, (srv, w, h, hx, hy));
                            }
                        }
                    }
                    if let Some((srv, w, h, hx, hy)) = cursor_cache.get(&cache_key) {
                        cursor_srv = Some(srv.clone());
                        cursor_w = *w;
                        cursor_h = *h;
                        hotspot_x = *hx;
                        hotspot_y = *hy;
                        has_cursor = true;
                    }
                }
            }

            // Fallback cursor position if GetCursorInfo failed
            if !has_cursor {
                let mut pt = POINT::default();
                if GetCursorPos(&mut pt).is_ok() {
                    current_mouse_x = pt.x as f32 - screen_x;
                    current_mouse_y = pt.y as f32 - screen_y;
                }
            }

            // Update trail queue
            trail.push_back(TrailPoint {
                x: current_mouse_x,
                y: current_mouse_y,
                time: Instant::now(),
            });
            // Keep trails alive for 0.3 seconds
            while !trail.is_empty() && trail.front().unwrap().time.elapsed().as_secs_f32() > 0.3 {
                trail.pop_front();
            }

            // Check click events from the mouse hook
            if let Some(mutex) = CLICK_EVENTS.get() {
                if let Ok(mut guard) = mutex.lock() {
                    for (x, y) in guard.drain(..) {
                        click_ripples.push(ClickRipple {
                            x: x - screen_x,
                            y: y - screen_y,
                            start_time: Instant::now(),
                            max_radius: 120.0, // Water wave radius in pixels
                            duration: 0.5,    // Fade out over 0.5s
                        });
                    }
                }
            }

            // Update click ripples lifetime
            click_ripples.retain(|ripple| ripple.start_time.elapsed().as_secs_f32() < ripple.duration);

            // =================================================================
            // Render Command Execution
            // =================================================================
            // Flip-model swap chains require binding the render target every frame after Present
            context.OMSetRenderTargets(Some(&[Some(render_target_view.clone())]), None);

            let clear_color: [f32; 4] = [0.0, 0.0, 0.0, 0.0]; // Transparent background
            context.ClearRenderTargetView(&render_target_view, &clear_color);

            // Setup pipelines
            let viewport = D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: screen_width,
                Height: screen_height,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            };
            context.RSSetViewports(Some(&[viewport]));
            context.OMSetBlendState(&blend_state, Some(&[0.0, 0.0, 0.0, 0.0]), 0xFFFFFFFF);
            
            let stride = size_of::<Vertex>() as u32;
            let offset = 0u32;
            let buffers = [Some(vertex_buffer.clone())];
            context.IASetVertexBuffers(
                0,
                1,
                Some(buffers.as_ptr()),
                Some(&stride as *const u32),
                Some(&offset as *const u32),
            );
            context.IASetInputLayout(&input_layout);
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            
            context.VSSetShader(&vertex_shader, None);
            context.VSSetConstantBuffers(0, Some(&[Some(constant_buffer.clone())]));
            
            context.PSSetShader(&pixel_shader, None);
            context.PSSetConstantBuffers(0, Some(&[Some(constant_buffer.clone())]));
            context.PSSetSamplers(0, Some(&[Some(sampler_state.clone())]));

            // Helper lambda to update constant buffer and render a quad
            let mut draw_quad = |rect_x: f32, rect_y: f32, rect_w: f32, rect_h: f32, color_rgba: [f32; 4]| {
                let mut mapped_resource = D3D11_MAPPED_SUBRESOURCE::default();
                let hr = context.Map(&constant_buffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped_resource));
                if hr.is_ok() {
                    let cb_ptr = mapped_resource.pData as *mut ConstantBufferData;
                    *cb_ptr = ConstantBufferData {
                        rect: [rect_x, rect_y, rect_w, rect_h],
                        screen_size: [screen_width, screen_height],
                        padding: [0.0, 0.0],
                        color: color_rgba,
                    };
                    context.Unmap(&constant_buffer, 0);
                    context.Draw(4, 0);
                }
            };

            // 1. Draw click wave ripples (expanding circular rings)
            context.PSSetShaderResources(0, Some(&[Some(circle_srv.clone())]));
            for ripple in &click_ripples {
                let elapsed = ripple.start_time.elapsed().as_secs_f32();
                let t = elapsed / ripple.duration; // normalized progress [0.0, 1.0]
                let current_radius = ripple.max_radius * t;
                let opacity = 1.0 - t; // fade out
                
                // Draw ripple outer circle
                let ripple_size = current_radius * 2.0;
                let ripple_x = ripple.x - current_radius;
                let ripple_y = ripple.y - current_radius;
                
                // Light cyan ripple color: RGBA: 0.2, 0.8, 1.0, opacity
                draw_quad(ripple_x, ripple_y, ripple_size, ripple_size, [0.2, 0.8, 1.0, opacity * 0.8]);
            }

            // 2. Draw mouse cursor trails (colored circular dots)
            context.PSSetShaderResources(0, Some(&[Some(circle_srv.clone())]));
            for (idx, pt) in trail.iter().enumerate() {
                let elapsed = pt.time.elapsed().as_secs_f32();
                let max_trail_time = 0.3;
                if elapsed < max_trail_time {
                    let t = elapsed / max_trail_time;
                    let size = 12.0 * (1.0 - t); // shrink over time
                    let opacity = 0.6 * (1.0 - t); // fade out over time
                    
                    let trail_x = pt.x - size / 2.0;
                    let trail_y = pt.y - size / 2.0;

                    // Colorful trail (changes color based on its index in queue)
                    let hue = (idx as f32 / 30.0) * 6.28;
                    let r = (hue.sin() * 0.5 + 0.5) * 1.0;
                    let g = ((hue + 2.09).sin() * 0.5 + 0.5) * 1.0;
                    let b = ((hue + 4.18).sin() * 0.5 + 0.5) * 1.0;
                    
                    draw_quad(trail_x, trail_y, size, size, [r, g, b, opacity]);
                }
            }

            // 3. Draw mouse cursor itself (if we got the texture and remote desktop is hiding it)
            if let Some(srv) = cursor_srv {
                context.PSSetShaderResources(0, Some(&[Some(srv)]));
                // Align using hotspot so the pointer tip lines up perfectly with physical coordinate
                let draw_x = current_mouse_x - hotspot_x as f32;
                let draw_y = current_mouse_y - hotspot_y as f32;
                
                // Render cursor at full opacity (1.0)
                draw_quad(draw_x, draw_y, cursor_w as f32, cursor_h as f32, [1.0, 1.0, 1.0, 1.0]);
            }

            // Commit rendering frame
            swap_chain.Present(1, DXGI_PRESENT(0)).ok()?;

            // Control frame rate: cap at ~120 FPS to reduce CPU utilization
            std::thread::sleep(Duration::from_millis(8));
        }
    }
}
