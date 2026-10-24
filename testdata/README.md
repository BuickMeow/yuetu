# 测试图片

这些是开发过程中生成的手动用例，体积都很小。可以用应用直接打开：

```bash
cargo run -p yuetu-app -- testdata/scrub30.gif
cargo run -p yuetu-app -- testdata/grid800.png
```

| 文件 | 尺寸 | 帧数 | 用途 |
|---|---:|---:|---|
| `grid800.png` | 800×800 | 1 | 原图长度/正方形像素、非正方形预览、旋转、键位映射 |
| `sparse800.png` | 800×800 | 1 | 透明背景、静音区域、稀疏音符、MIDI 小节线 |
| `ruler-test.png` | 800×600 | 1 | 标尺、旋转、左下角原点 |
| `frames40.gif` | 240×160 | 40 | 帧条缩略图/时间轴、普通动画 |
| `frames300.gif` | 160×100 | 300 | 长动画时间轴、选帧滚动 |
| `shared-palette.gif` | 300×200 | 2 | 多帧共享调色板 |
| `scrub30.gif` | 800×800 | 30 | 拖动扫帧、横向时间、固定高度、预览比例 |

解码回归测试：

```bash
cargo test -p yuetu-core --test testdata
```
