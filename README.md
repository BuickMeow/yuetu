# Yuetu

把图片当作钢琴卷帘，转换成 MIDI。使用 **Rust + egui 0.37 + wgpu**。

当前是可运行的第一阶段（M1 + 后台转换任务）：

- 打开 / 拖入图片，缩放、平移查看原图；
- GIF / 动态 WebP 逐帧选择，切帧后自动重新生成预览；
- 自动生成 popularity 调色板（每色一条音轨）；
- 图片宽度映射到选定的 MIDI 键范围，逐行扫描生成音符；
- 后台线程转换，带分阶段进度条，可随时取消（Esc）；
- 打开图片后自动生成；任何转换参数变化都会自动重新生成预览，不需要手动点“生成”；
- 触控板双指滚动平移（上下左右）、捏合缩放；鼠标滚轮缩放；方向键平移；
- 左右面板使用自定义 split handle，拖动宽度、双击恢复默认；
- 设置卡片统一为“名称在左、控件在右”，卡片标题和操作同一行；
- 右侧实时显示钢琴卷帘预览（横轴音高、纵轴时间）；
- 预览音符合并为单个 `egui::Mesh`；
- 导出标准 Format-1 `.mid`，可选写入原版的颜色 Meta 事件。

> 这一步的重点不是功能多，而是把**数据边界**先定下来：
> `RgbaImage` → `Palette/KeyMap` → `IndexedImage` → `ConversionResult` → UI/MIDI。
> 预览只读 `ConversionResult`，不依赖图片控件，也不依赖 wgpu 细节。

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

## 目录

```text
crates/
  yuetu-core/          # 纯领域层：无 UI、无窗口、无 egui
    image.rs           #   RgbaImage + 文件解码
    resize.rs          #   Area / Nearest
    palette.rs         #   自动调色板 + 最近色
    quantize.rs        #   像素 → 调色板索引
    keymap.rs          #   键过滤 + 列映射
    notegen.rs         #   唯一的音符生成算法（纯函数）
    convert.rs         #   流水线编排 + ConversionResult
    midi.rs            #   Format-1 MIDI 写出
  yuetu-app/           # 表现层：eframe + egui
    app.rs             #   状态与动作编排
    settings.rs        #   UI 参数 → core 配置
    state.rs           #   SourceImage / Preview / 状态消息
    ui/                #   面板拆分（toolbar/controls/preview/status）
    widgets/           #   可复用控件（图片视图、钢琴卷帘、View2D）
    theme.rs           #   主题
```

详细设计见 [`docs/architecture.md`](docs/architecture.md)。

## 当前故意没做的事

- 调色板只有 popularity 一种算法。后续按同一 `Palette` 接口加入 K-Means、Octree、抖动等。
- 预览的音符已经合并成一个 `egui::Mesh`（不再每个音符一个 Shape）；只有经过性能测量、确实需要时才下沉到自定义 wgpu instancing pipeline。
- 视频解码尚未接入。GIF/动态 WebP 已支持逐帧选择；视频需要单独的 ffmpeg 或原生解码导入层。
- 手动调色板 / 原图取色替换尚未接入 UI。

## HiDPI

由 eframe / egui / wgpu 自动处理：窗口、字体、控件、分割线、预览坐标都以逻辑点为单位，纹理使用实际像素尺寸。Retina 2x 下已验证 UI 和图片显示清晰，不需要额外的 1x/2x 分支。

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
