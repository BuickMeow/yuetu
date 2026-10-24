# 架构：先竖切，再横向铺功能

## 1. 为什么第一步不是“先写两个控件”

如果先分别做图片控件和 MIDI 预览控件，中间没有真实数据，最后一定会出现两种坏结果：

1. 预览控件自己去读图片 / 调色板 / 配置，职责再次粘在一起；
2. 等转换器接进来时，发现预览需要的数据结构和转换器产出的东西对不上，只能加适配层和临时字段。

所以第一条竖切是：

```text
加载图片 → 真实转换 → ConversionResult → 图片显示 + 预览显示 → 导出 MIDI
```

两个显示功能一起做，但**中间用显式数据类型隔开**。这样任何一端都可以独立替换。

---

## 2. 分层与依赖方向

```text
┌─────────────────────────────────────────────┐
│ yuetu-app (eframe/egui)                     │
│  AppState → Panel → Action → App 编排        │
│  SourceImage / Preview / Status             │
│  widgets: View2D / ImageView / MidiPreview  │
└───────────────────┬─────────────────────────┘
                    │ 只依赖公开数据类型
┌───────────────────▼─────────────────────────┐
│ yuetu-core (纯 Rust)                        │
│  image → resize → palette → quantize        │
│  keymap + notegen → ConversionResult        │
│  midi writer                                │
└─────────────────────────────────────────────┘
```

依赖只能向下。`yuetu-core` 不允许出现 `egui`、`wgpu`、`rfd`、窗口或线程 UI 状态。

---

## 3. 数据流

### 3.1 输入

```rust
ImageSequence {
    frames: Vec<RgbaImage>, // 静态图只有一个元素
    delays_ms: Vec<u32>,    // GIF/WebP 帧延迟
}
```

图片解码只负责得到像素。GIF/WebP 拆帧和选帧属于前处理；动画帧在 UI 线程一次性解码并设置内存上限。`convert_frames_with_progress()` 接收当前选中的那一帧，以及完整帧序列/帧延迟用于共享调色板；动图没有“当前帧色板”选项，所有帧必须共用一套配色。旋转是 `ConversionConfig.rotation_deg`，在 worker 里先于缩放执行。源图阶段只对显示分辨率（受 GPU 纹理上限约束）的副本做旋转并上传；全分辨率旋转只发生在 worker，避免拖动角度时卡 UI。标尺和命中使用完整旋转后尺寸，颜色拾取按预览比例映射。

### 3.2 中间表示

```rust
Palette { colors: Vec<Color> }
KeyMap { columns: Vec<Option<u8>> }   // 列号 → MIDI 键（None = 静音列）
IndexedImage { width, height, indices: Vec<i32> } // -2 = 静音
```

`IndexedImage` 是前处理与核心算法之间唯一的契约。核心算法不关心：
- 图片从哪来；
- 用了什么缩放算法；
- 调色板是自动聚类还是手动选的；
- 输出 MIDI 的 PPQ / BPM。

自动调色板先统计加权 RGB 直方图：动画的每一帧按帧延迟加权，透明像素跳过；然后用加权 K-Means++ 初始化 + 加权 K-Means 聚类。这样所有帧共用一组颜色和音轨索引，避免切帧时同一颜色跳到另一条轨。颜色距离使用 RGB 平方距离，与后续 `nearest_index()` 的量化距离保持一致。

### 3.3 输出

```rust
ConversionResult {
    notes: Vec<Note>,          // key, track, start, end（单位：pixel tick）
    palette: Palette,
    keymap: KeyMap,
    duration_px: u32,
    source_size / resized_size,
    resized_image: RgbaImage,  // 键位映射阶段的图片
    indexed_image: IndexedImage, // 调色板阶段的索引色图
    note_count_by_track,
}
```

`ConversionResult` 同时是：
- 钢琴卷帘预览的唯一输入；
- MIDI writer 的唯一输入；
- 以后播放/试听、批量导出、缓存、撤销重做的快照单位。

`Note` 故意不存颜色，只存 `track`。颜色属于 `Palette`，这样同一份音符可以换调色板重新着色，而不用重新扫描图片。

---

## 4. 转换算法边界

`convert()` 只做编排：

```text
validate config
  → KeyMap::build
  → rotate(rotation_deg)
  → Palette::from_frames（动图所有帧加权 / 手动调色板）
  → resize(width = keymap.width, height = aspect)
  → quantize
  → generate_notes
  → ConversionResult
```

真正的核心算法只有 `notegen::generate_notes`：

- 从图片底部向上扫描；
- 每一行推进 1 个 pixel-tick；
- 每一列的颜色变化时，关闭旧音符、打开新音符；
- 支持“持有时长重触发”和“固定网格切分”两种长度限制；
- 纯函数，输入输出都是数据，可直接单元测试。

音符扫描中只遍历键映射真正映射到的列，固定网格的重触发条件每行只计算一次，输出使用非稳定排序；这些改动不改变结果，只减少每像素的重复工作。

本机合成基准（128 列、163 行、16 色）中，缩放约 1.9 ms、量化约 0.23 ms、音符生成约 0.12 ms，而 30 帧 512×512 的全帧调色板聚类约 18 ms。动画切帧时的主要浪费来自重复聚类，不在音符生成本身；共享色板缓存解决的是这部分。

以后加入并行量化、取消、进度，不会改变这个函数的签名。

---

## 5. UI 组织

### 5.1 状态

`YuetuApp` 只持有一棵状态树：

```text
source: Option<SourceImage>   // 全部帧 + 当前帧 CPU 图 + 旋转显示图 + GPU 纹理
source_revision: u64          // 图片、GIF 帧或旋转角度变化时递增
preview: Option<Preview>      // ConversionResult + 配置 + 阶段纹理
settings: Settings            // UI 参数（含旋转、颜色数）
stage: PipelineStage          // 当前流水线阶段
stage_stops: Vec<StageStop>   // 左侧卡片中心，供图标滑块吸附
coordinate_origin             // 标尺/悬停坐标原点：左上角或左下角
frame_strip_last_frame        // 帧条上次选中的帧，用于外部选帧时自动滚入视野
source/key/palette/midi_view  // 每个阶段独立的 View2D
status / hover                // 一帧内的展示信息
```

### 5.2 面板与动作

每个面板是一个纯函数：

```rust
fn show(ui, 状态引用, 可变 UI 状态) -> Action
```

面板不直接改 `YuetuApp` 的全局状态，只返回动作或绘制信息：

```text
controls::Action   { open, cancel, export, frame, rotation }
stage_panel::Action { open }
status_bar::Action  { set_zoom_percent, fit_width }
```

`app.rs` 统一消费动作，调用文件对话框、切换 GIF 帧、取消任务、导出器或调整当前阶段视图。自绘顶栏已移除，窗口标题直接使用“月兔”。

设置项统一使用“名称在左、控件右对齐”的行布局，卡片标题与卡片级操作也使用同一行。`controls::show` 返回四个卡片的屏幕中心，供最左侧图标轨道吸附。

底部状态栏使用 `egui_extras::StripBuilder` 固定列宽（阶段名 80px、缩放编辑器 64px、循环按钮 64px），行高固定为 26px。底部面板本身不再加 top margin：中央面板已经有 8px bottom margin，避免两段同色 padding 叠加成 16px 的视觉空隙。

转换不再由面板按钮触发。`app.rs` 在每帧 UI 结束后比较 `ConversionConfig`：只要有变化，就经过 80 ms debounce 启动后台任务；GIF/WebP 选帧同样会递增 `source_revision` 并触发更新。拖拽滑杆时不会每帧重启任务，但松手后立即更新。

### 5.3 后台转换任务

耗时转换不占用 UI 线程：

```text
UI: JobRunner::start(image Arc + palette frames + delays + config)
        │
        ▼
worker thread: convert_frames_with_progress(..., cancel, progress)
        │  JobEvent::Progress / Finished / Failed / Cancelled
        ▼
UI: poll_jobs() → 状态栏 + 进度条 + 新 Preview
```

- `Arc<RgbaImage>` 避免复制大图；
- `AtomicBool` 做协作式取消，核心在每个 stage / 每 16 行检查一次；
- 进度按 stage 加权成 0–100%，而不是把不同阶段的 0–1 直接混在一起；
- 同一时间只跑一个任务；启动新任务会取消旧任务，并按 job id 丢弃旧事件；
- 参数变化会自动把新任务排到 80 ms 后；旧结果即使先返回，也只会短暂作为占位，更新状态由进度条表示，不再出现“预览已过期”；
- 导出的前提是 `Preview.config == 当前 ConversionConfig` 且 `Preview.source_revision == 当前 source_revision`，避免在自动更新或切帧途中导出旧结果。
- 共享调色板在 UI 侧缓存：key 是 `source_generation + palette_colors`。切帧、旋转不会让“所有帧”色板失效，所以时间轴拖动时只重跑当前帧的缩放/量化/音符；只有换图或改色板设置才重新聚类。
- 四个阶段仍然在同一个后台任务里顺序执行，`ConversionResult` 是原子快照；目前缓存的是跨阶段共享的调色板，尚未做 resized/indexed/notes 的分段缓存。

### 5.4 主题开关

`theme.rs` 提供 `CUSTOM_THEME` 开关：

- `false`：使用 egui 内置 dark 主题，所有颜色通过 `ui.visuals()` 派生；
- `true`：恢复 Yuetu 的 graphite/mint 配色。

面板、卡片、视图不直接写自定义色值，只调用 `theme::panel_frame` / `card_frame` / `view_frame` 和颜色辅助函数。这样主题切换不影响任何布局逻辑，也方便后面做真正的主题选择器。

### 5.5 一个视图，四个 View2D 状态

界面上只有一个中央视图，但 `Source` / `KeyMap` / `Palette` / `Midi` 各保留自己的缩放/平移状态，切阶段不会丢失视角：

- 滚轮：以光标位置为锚点缩放；鼠标滚轮的事件来源会被记住到这次滚动结束，避免平滑滚动的尾帧变成平移；
- 缩放锚点在一次手势开始时记录为内容坐标（内部用 f64），后续亚像素缩放不会因为反复用 f32 pan 重新锚定而漂移；
- 到达绝对缩放上下限（1%–6400%，并受相对上限约束）时缩放直接 no-op，不重新计算 pan；亚像素步长舍入到同一个 f32 zoom 时同样不更新 pan；
- 捏合：以指针位置为中心缩放；活跃捏合期间不会同时叠加滚动手势的平移；
- 左键拖拽 / 触控板双指：平移；
- 双击：适配窗口；
- 状态栏百分比是绝对缩放（100% = 一个图片像素对应一个逻辑点），可拖动编辑；旁边是一个循环缩放按钮：适应（完整显示）→ 填充（覆盖视口并裁剪）→ 100%。

MIDI 阶段的坐标单位是“列 × pixel-tick”，不是屏幕像素；因此窗口缩放、面板拖拽都不会改变数据坐标。

### 5.6 流水线阶段与轨道

四个阶段由左侧卡片定义：

1. `源图`：原始图像 + Photoshop 风格像素标尺；可设旋转角度（正值顺时针），标尺原点可切到左下角；
2. `键位映射`：按 `KeyMap::width()` 缩放后的图片，保持宽高比；
3. `调色板`：`IndexedImage` 按调色板还原成索引色图，悬停显示索引和色号；动图始终使用所有帧的共享色板；
4. `MIDI 生成`：钢琴卷帘音符；音符长度/切分设置也归入这一阶段。

标尺左上角是明确的 corner cell（横/纵标尺交汇格），不是漏画；它的右边和下边与标尺分隔线相连。坐标原点为左下角时，纵轴标签按 `height - 1 - row` 显示，底部像素行为 0，顶部为 `height - 1`；状态栏悬停坐标同步使用同一转换。

最左侧保留 32px 轨道：灰色长圆角矩形宽 24px、圆角 12px，与高亮圆的直径/半径完全一致；轨道两端额外延长一个半径，使端帽中心与首尾阶段中心重合，当前阶段的圆形 thumb 能完整覆盖端帽。每个阶段中心放一个 Material Icon，当前阶段用同一个 24px 圆形 thumb 包裹图标。轨道颜色向面板背景混合，保持低对比。拖拽时按指针 y 找最近卡片中心并吸附，不会停在卡片之间。

空状态（“把图片拖进来” / 选择图片）使用 `egui_extras::StripBuilder` 的垂直 remainder/exact/remainder 三段布局做整体居中。

左侧面板和中央面板的 content 顶部边距都统一为 8px；底部栏自身不再加 top margin，由中央面板的 bottom margin 提供视觉间距。

### 5.7 动图帧条

`SourceImage.frame_count() > 1` 时，中央视图底部出现一条横向帧条（内嵌在 CentralPanel 内，因此只占主视图宽度，不跨左侧面板）。帧条右侧有一个模式按钮，手动切换缩略图/时间轴；默认缩略图：

- 默认显示缩略图：每帧一个固定宽度 cell，显示缩略图、帧号；
- 右侧模式按钮可切换到紧凑时间轴，显示连续帧刻度、每 5/10/... 帧的编号、当前帧色带和播放头；再点一次切回缩略图；切换模式后自动滚到当前帧；
- 两种模式都支持点击选帧；时间轴还支持按住拖动播放头扫帧；拖动期间源图始终更新，键位映射/调色板阶段会用当前缩放算法重建临时图像（不跑完整转换），有旋转时直接复用已经旋转好的显示预览，MIDI 等最终帧松手后再更新；
- 当前帧不画边框，用底部/整格色带和播放头区分；
- 帧数超出宽度时显示横向滚动条；短动画不显示滚动条；
- 缩略图按需生成（只生成可见 cell），旋转变化时清空缓存；
- 外部选帧（例如 DragValue 跳到第 3000 帧）时，帧条自动把该帧滚入视野；
- 点击和拖动都走 `select_source_frame()`，会递增 `source_revision` 并自动重生成预览。

### 5.8 HiDPI

eframe / egui / wgpu 已经处理窗口的 `pixels_per_point`：布局、字体、描边、标尺、命中区和四个 `View2D` 全部使用逻辑点，上传给 GPU 的纹理使用图像实际像素。唯一相关的是从 `ctx.input(|i| i.max_texture_side)` 读取纹理上限，避免超过后端限制。Retina 2x 下已验证。

---

## 6. wgpu 当前边界与音符性能

```text
eframe::Renderer::Wgpu
  ├─ egui UI / 标尺 / 网格：tessellation → wgpu
  ├─ 源图 / 键位图 / 索引色图：单个 egui 纹理 quad → wgpu
  ├─ 透明棋盘格：2×2 Repeat 纹理 + 单个 quad → GPU 平铺
  └─ MIDI 音符：egui_wgpu::Callback + instance buffer → 一次 instanced draw
```

- 图片显示本来就是一个纹理 quad，GPU 采样；不需要每帧 CPU 重建像素。
- 棋盘格改为一张可重复的 2×2 灰度纹理，通过 `TextureWrapMode::Repeat` 和单个 Mesh quad 绘制；不再逐格生成 Shape。
- MIDI 音符实例在 `ConversionResult` 变化时构建并上传一次；拖动/缩放只更新 uniform，GPU 每帧一次 instanced draw。实例数据包含列、start/end、颜色，顶点 shader 直接生成 note quad。

大图高倍缩放曾经的主要卡顿源是棋盘背景逐格生成；现在所有阶段纹理和音符都在 GPU 批量绘制。5000×5000 约 1900% 缩放实测可交互。`ConversionResult` 契约不变，替换范围仍被限制在 phase widgets 内。

---

## 7. 后续迭代

### M2：可用性

- [x] 转换移到后台线程：`JobRunner` + `JobEvent` + 分阶段进度 + 协作式取消；
- [x] 参数变化自动重新生成预览（80 ms debounce，无手动生成按钮）；
- [x] GIF / 动态 WebP 逐帧选择，切帧自动更新预览；
- [x] 主视图下方横向帧条：缩略图/时间轴切换按钮、点击/拖动选帧、当前帧色带、横向滚动、按需生成缩略图；
- [x] 多帧共享调色板：按帧延迟加权直方图 + 加权 K-Means++；
- [x] 共享调色板缓存：切帧/旋转不重复聚类；
- [x] 帧拖动期间的键位映射/调色板临时预览（跳过音符生成）；
- [x] 音符生成列投影、网格重触发预计算、非稳定排序；
- [x] 原图旋转（任意角度，源图阶段使用显示分辨率预览，转换在 worker 中执行）；
- [x] 标尺/悬停坐标原点切换（左上角 / 左下角）；
- [x] 左侧 Material Icons 流水线 + 四阶段中央视图（源图/键位/索引色/MIDI）；
- [ ] 颜色拾取：点原图取色，编辑/替换调色板；
- [ ] 手动调色板 + 自动调色板切换；
- [ ] 打开最近文件、窗口状态持久化。

### M3：功能对齐 ImageToMidi_Modded

- 调色板算法：Octree、Median Cut、GMM、DBSCAN、Floyd/Ordered dithering；
- 缩放算法：Bilinear、Bicubic、Lanczos、Area、Nearest、Mode pooling；
- 高度模式、黑/白键六种组合、最大音符长度模式；
- 视频导入与选帧（ffmpeg / 原生解码后端）；解码后的帧和时长直接复用 `Palette::from_frames_cancellable()` 与 `convert_frames_with_progress()`；
- SVG/PSD/TIFF 等格式；
- 批量导出；
- 中英文界面。

### M4：性能与工程化

- 预览 LOD：按缩放级别合并音符，限制每帧 shape 数；
- [x] 透明棋盘格 GPU 平铺；
- [x] MIDI 音符自定义 wgpu instanced pipeline；
- [ ] 必要时做 GPU culling / LOD；
- 转换缓存（图片 hash + 配置 hash）；
- 端到端快照测试（无头 egui_kittest）。

---

## 8. 代码规矩

1. **core 不依赖 UI**。需要窗口、文件对话框、纹理时，说明代码放错层了。
2. **一个模块只做一类事**。`resize` 不生成调色板，`palette` 不生成音符，`notegen` 不写 MIDI。
3. **中间结果必须有类型**。不要用 `Vec<Vec<u8>>` 或一堆临时 bool 在函数之间传递阶段状态。
4. **配置按阶段分组**。UI 的 `Settings` 可以平铺方便绑定，但进入 core 前映射成 `ConversionConfig` / `MidiOutputConfig`。
5. **先测量再优化 GPU**。预览是数据可视化，先保证坐标和交互正确，再谈实例化。
