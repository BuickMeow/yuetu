# Yuetu

把图片当作钢琴卷帘，转换成 MIDI。使用 **Rust + egui 0.37 + wgpu**。

当前是可运行的第一阶段（M1 + 后台转换任务）：

- 打开 / 拖入图片，缩放、平移查看原图；
- GIF / 动态 WebP 逐帧选择，切帧后自动重新生成；
- 无自绘顶栏，窗口标题直接为“月兔”；
- 最左侧 Material Icons 流水线滑块沿卡片中心停靠，切换四个处理阶段；
- 阶段视图：源图（像素标尺）→ 键位映射（按键宽缩放后的图片）→ 调色板（索引色）→ MIDI 生成（钢琴卷帘音符）；
- 底部状态栏提供缩放百分比拖动编辑，以及“适应 → 填充 → 100%”循环按钮；
- 自动生成 popularity 调色板（每色一条音轨）；
- 图片宽度映射到选定的 MIDI 键范围，逐行扫描生成音符；
- 后台线程转换，带分阶段进度条，可随时取消（Esc）；
- 打开图片后自动生成；任何转换参数变化都会自动重新生成，不需要手动点“生成”；
- 触控板双指滚动平移（上下左右）、捏合缩放；鼠标滚轮只缩放；方向键平移；
- 左侧面板使用自定义 split handle，拖动宽度、双击恢复默认；
- 设置卡片统一为“名称在左、控件在右”，卡片标题和操作同一行；
- 图片本身是单个 GPU 纹理 quad；透明棋盘格是一张可重复纹理，一个 quad 由 GPU 平铺；
- MIDI 音符使用自定义 `egui_wgpu::Callback` + 持久 instance buffer，一次 instanced draw 完成；
- 导出标准 Format-1 `.mid`，可选写入原版的颜色 Meta 事件。

> 这一步的重点不是功能多，而是把**数据边界**先定下来：
> `RgbaImage` → `Palette/KeyMap` → `ResizedImage/IndexedImage` → `ConversionResult` → UI/MIDI。
> 所有阶段视图都只读 `ConversionResult` 或源图，不依赖 wgpu 细节。

## 运行

要求 Rust stable（当前 1.99+）。

```bash
cargo run -p yuetu-app --release
# 或直接带一张图启动
cargo run -p yuetu-app --release -- path/to/image.png
```

开发构建：

```bash
cargo run -p yuetu-app
```

## 测试图片

`testdata/` 收了一组小型手动用例：静态网格、透明稀疏图、40/300 帧动画、共享调色板动画和 800×800 扫帧动画。说明见 [`testdata/README.md`](testdata/README.md)，解码回归：

```bash
cargo test -p yuetu-core --test testdata
```

## 目录

```text
crates/
  yuetu-core/          # 纯领域层：无 UI、无窗口、无 egui
    image.rs           #   RgbaImage + 文件解码
    resize.rs          #   Area / Nearest
    palette.rs         #   自动调色板 + 最近色
    quantize.rs        #   像素 → 调色板索引
    keymap.rs          #   键过滤 + 键位分布
    notegen.rs         #   唯一的音符生成算法（纯函数）
    convert.rs         #   流水线编排 + ConversionResult
    midi.rs            #   Format-1 MIDI 写出
  yuetu-app/           # 表现层：eframe + egui
    app.rs             #   状态与动作编排
    settings.rs        #   UI 参数 → core 配置
    state.rs           #   SourceImage / Preview / PipelineStage / 状态消息
    ui/                #   面板拆分（controls/stage/status）
    widgets/           #   图标流水线、阶段图片、GPU 音符渲染、View2D
    theme.rs           #   主题
```

详细设计见 [`docs/architecture.md`](docs/architecture.md)。

## 当前故意没做的事

- 调色板只有 popularity 一种算法。后续按同一 `Palette` 接口加入 K-Means、Octree、抖动等。
- 图片显示已经是 GPU 纹理采样，不需要每帧重建像素；大图只在加载时上传/降采样纹理。
- 透明棋盘格是一张 2×2 可重复纹理，单个 quad 由 GPU 平铺；不透明图直接跳过。
- MIDI 音符使用 wgpu instance buffer，只在 `ConversionResult` 变化时上传，缩放/平移只改 uniform；5000×5000 图片在约 1900% 缩放下仍可交互。
- 视频解码尚未接入。GIF/动态 WebP 已支持逐帧选择；视频需要单独的 ffmpeg 或原生解码导入层。
- 手动调色板 / 原图取色替换尚未接入 UI。

## HiDPI

由 eframe / egui / wgpu 自动处理：窗口、字体、控件、分割线、标尺、预览坐标都以逻辑点为单位，纹理使用实际像素尺寸。Retina 2x 下已验证 UI 和图片显示清晰，不需要额外的 1x/2x 分支。

## 主题

`crates/yuetu-app/src/theme.rs` 中：

```rust
pub const CUSTOM_THEME: bool = false;
pub const THEME_PREFERENCE: egui::ThemePreference = egui::ThemePreference::Dark;
```

当前为 `false`，使用 egui 0.37 内置 dark 主题；把 `THEME_PREFERENCE` 改成 `Light` 或 `System` 可切换内置主题。把 `CUSTOM_THEME` 改成 `true` 恢复之前的深石墨 + 薄荷绿自定义配色。所有面板、卡片、视图和文字颜色都通过 `theme::` 辅助函数取色，布局代码无需修改。

## 参考

- `../ImageToMidi`：原始 C# 实现
- `../ImageToMidi_Modded`：功能扩展版（本项目的功能参照）
- `../yinhe`：egui/wgpu 工程组织与自定义渲染参考
