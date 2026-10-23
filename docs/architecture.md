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
RgbaImage {
    width: u32,
    height: u32,
    pixels: Vec<u8>, // row-major RGBA
}
```

图片解码只负责得到像素。旋转、翻转、灰度、GIF 拆帧以后都属于“前处理”，但不应塞进 `convert()`。

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

### 3.3 输出

```rust
ConversionResult {
    notes: Vec<Note>,          // key, track, start, end（单位：pixel tick）
    palette: Palette,
    keymap: KeyMap,
    duration_px: u32,
    source_size / resized_size,
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
  → Palette::from_image（或手动调色板）
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

以后加入并行量化、取消、进度，不会改变这个函数的签名。

---

## 5. UI 组织

### 5.1 状态

`YuetuApp` 只持有一棵状态树：

```text
source: Option<SourceImage>   // CPU 全分辨率图 + GPU 显示纹理
preview: Option<Preview>      // ConversionResult + 产生它的配置
settings: Settings            // UI 参数
image_view / preview_view     // 两个 View2D
status / hover                // 一帧内的展示信息
```

### 5.2 面板与动作

每个面板是一个纯函数：

```rust
fn show(ui, 状态引用, 可变 UI 状态) -> Action
```

面板不直接改 `YuetuApp` 的全局状态，只返回 `Action { open, cancel, export, fit }`。`app.rs` 统一消费动作，调用文件对话框、取消任务、导出器。

转换不再由面板按钮触发。`app.rs` 在每帧 UI 结束后比较 `ConversionConfig`：只要有变化，就经过 80 ms debounce 启动后台任务；拖拽滑杆时不会每帧重启任务，但松手后立即更新。

### 5.3 后台转换任务

耗时转换不占用 UI 线程：

```text
UI: JobRunner::start(image Arc + config)
        │
        ▼
worker thread: convert_with_progress(..., cancel, progress)
        │  JobEvent::Progress / Finished / Failed / Cancelled
        ▼
UI: poll_jobs() → 状态栏 + 进度条 + 新 Preview
```

- `Arc<RgbaImage>` 避免复制大图；
- `AtomicBool` 做协作式取消，核心在每个 stage / 每 16 行检查一次；
- 进度按 stage 加权成 0–100%，而不是把不同阶段的 0–1 直接混在一起；
- 同一时间只跑一个任务；启动新任务会取消旧任务，并按 job id 丢弃旧事件；
- 参数变化会自动把新任务排到 80 ms 后；旧结果即使先返回，也只会短暂作为占位，更新状态由进度条表示，不再出现“预览已过期”；
- 导出的前提是 `Preview.config == 当前 ConversionConfig`，避免在自动更新途中导出旧结果。

### 5.4 主题开关

`theme.rs` 提供 `CUSTOM_THEME` 开关：

- `false`：使用 egui 内置 dark 主题，所有颜色通过 `ui.visuals()` 派生；
- `true`：恢复 Yuetu 的 graphite/mint 配色。

面板、卡片、视图不直接写自定义色值，只调用 `theme::panel_frame` / `card_frame` / `view_frame` 和颜色辅助函数。这样主题切换不影响任何布局逻辑，也方便后面做真正的主题选择器。

### 5.5 两个 View2D

图片视图和预览视图共用同一套缩放/平移数学：

- 滚轮 / 捏合：以鼠标位置为中心缩放；
- 左键拖拽：平移；
- 双击：适配窗口；
- `fit` 是“相对缩放 1.0”，UI 状态栏显示百分比。

预览的坐标单位是“列 × pixel-tick”，不是屏幕像素。因此窗口缩放、面板拖拽都不会改变数据坐标。

---

## 6. wgpu 当前边界与音符性能

```text
eframe::Renderer::Wgpu
  ├─ egui 全部 UI 的三角形上传 / 绘制
  ├─ 原图：egui::TextureHandle → wgpu 纹理
  └─ MIDI 预览：egui::Mesh（一个批次）→ wgpu
```

目前没有自定义 wgpu pipeline。音符先合并成单个 `egui::Mesh`，避免“每个音符一个 Shape + 一次 tessellation”的 CPU 开销。GPU 填充这些矩形本身很便宜；真正的瓶颈会先出现在 CPU 生成顶点和每帧上传顶点。

当音符数量达到数十万、或需要 LOD/GPU culling 时，再把 `midi_preview` 换成自定义 `egui_wgpu::Callback`：

- 音符实例只在数据变化时上传；
- 缩放/平移通过 uniform 完成；
- 视口裁剪在 GPU 做；
- 预览渲染到离屏纹理，再由 egui 显示。

`ConversionResult` 契约不变，替换范围仍然在一个 widget 内。

---

## 7. 后续迭代

### M2：可用性

- [x] 转换移到后台线程：`JobRunner` + `JobEvent` + 分阶段进度 + 协作式取消；
- [x] 参数变化自动重新生成预览（80 ms debounce，无手动生成按钮）；
- [ ] 颜色拾取：点原图取色，编辑/替换调色板；
- [ ] 手动调色板 + 自动调色板切换；
- [ ] 打开最近文件、窗口状态持久化。

### M3：功能对齐 ImageToMidi_Modded

- 调色板算法：K-Means++、Octree、Median Cut、GMM、DBSCAN、Floyd/Ordered dithering；
- 缩放算法：Bilinear、Bicubic、Lanczos、Area、Nearest、Mode pooling；
- 高度模式、黑/白键六种组合、最大音符长度模式；
- GIF/WebP 帧导航、SVG/PSD/TIFF 等格式；
- 批量导出；
- 中英文界面。

### M4：性能与工程化

- 预览 LOD：按缩放级别合并音符，限制每帧 shape 数；
- 必要时自定义 wgpu pipeline / compute culling；
- 转换缓存（图片 hash + 配置 hash）；
- 端到端快照测试（无头 egui_kittest）。

---

## 8. 代码规矩

1. **core 不依赖 UI**。需要窗口、文件对话框、纹理时，说明代码放错层了。
2. **一个模块只做一类事**。`resize` 不生成调色板，`palette` 不生成音符，`notegen` 不写 MIDI。
3. **中间结果必须有类型**。不要用 `Vec<Vec<u8>>` 或一堆临时 bool 在函数之间传递阶段状态。
4. **配置按阶段分组**。UI 的 `Settings` 可以平铺方便绑定，但进入 core 前映射成 `ConversionConfig` / `MidiOutputConfig`。
5. **先测量再优化 GPU**。预览是数据可视化，先保证坐标和交互正确，再谈实例化。
