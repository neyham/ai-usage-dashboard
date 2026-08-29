# AI Usage Dashboard — agent notes

## README 展示图（换图只做这件事）

用户说换 README 图 = **覆盖这三张 PNG**，不是写新脚本、不是改测试。

| 文件 | 画面 |
|------|------|
| `docs/assets/dashboard.png` | 默认 IP 皮肤，**四面板 2×2 横向卡**（Codex / Claude / DeepSeek / Grok，条形用量） |
| `docs/assets/dashboard-six-rings.png` | IP 皮肤，**六宫格圆环** |
| `docs/assets/dashboard-eva.png` | EVA 皮肤，六面板分段条 |

做法：

1. 合成数据（`--judge-demo` / 现有 mock）。禁止真账号、路径、token 入镜。
2. 覆盖上面三个路径（文件名不要改）。
3. 布局变了再改 README alt，没变就不动文案。
4. 提交只要 PNG（+ 必要的 README）。**不要**改 `scripts/viewport-check.mjs`，不要新增 capture 脚本，不要新 npm script。

已有 `npm run test:ui` 末尾会写这三张；那是副作用。换图若顺带跑测试可以，但 git 里只提交图，不提交 harness 改动。
