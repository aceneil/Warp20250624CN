# UI 参考：WARP 1.5.3 主窗口（照抄结构，不抄皮肤）

来源：用户提供的 `Win App Revamp Package 1.5.3` 截图（1520×1145）。
**结论先行：布局与交互照抄；配色、圆角、发光、图标一律用本项目的深色 token。**

## 分区（自上而下）

| # | 区域 | 高度占比 | 内容 |
|---|------|---------|------|
| 1 | 顶栏 | ~6% | 左侧列表图标；**居中**两枚胶囊徽标：`Loaded Date (None)`、`Selected Patches (0)`；右侧拼图图标 |
| 2 | Source 行 | ~7% | 标签在输入框左上边框上（fieldset 风格），输入框内是完整路径，右端两个小按钮：清空 / 选文件 |
| 3 | Target 行 | ~7% | 同上，输出路径 |
| 4 | 过滤行 | ~6% | 左：放大镜 + 占位符 `Filter Expression`；右：`Sort A→Z by Title` |
| 5 | Patch List | ~55% | 大圆角框，标题「Patch List」压在边框上；内部是列表（截图里为空） |
| 6 | Quick Actions | 与 5 同高 | 右侧竖排 5 个等宽按钮，两个词换行：`Load Source` / `Select Recommended` / `Select Previous` / `Apply Patches` / `Load Scripts` |
| 7 | 底栏 | ~6% | 左 `Style: Default`；中间 5 个圆形图标按钮；右 `Language: Default (ENG)` |

## 我们的对应实现

| WARP | warp_gui（Tauri） |
|---|---|
| Loaded Date | 补丁表版本 / 载入时间（`patches/*.yml` 的 mtime + 条目数） |
| Selected Patches | 已勾选条数 / 总数 |
| Source | 客户端 exe（拖拽 + 浏览），右侧显示大小与 md5 |
| Target | `<原名>_patched.exe`，可改 |
| Filter Expression | 按 id / 说明 / 偏移实时过滤 |
| Sort by Title | 按 id、按偏移、按状态（命中/跳过）三种排序 |
| Patch List | **表格**：勾选列 / id / 说明 / 模式 / 偏移 / 在当前 exe 上是否可定位 |
| Load Source | 载入补丁表（扫描 `patches/*.yml`） |
| Select Recommended | 选推荐集（`base` + `custom_lub`） |
| Select Previous | 恢复上次勾选（记在本地配置里） |
| Apply Patches | 预览（dry-run）与应用（自动备份 + verify） |
| Load Scripts | 载入自定义 yml |

## 皮肤红线
- ❌ 不用紫色渐变顶栏、不用白底 + 紫色描边、不用发光/毛玻璃。
- ✅ 深色底（`#14161a` 系）、单色强调、1px 描边、8px 圆角；
- ✅ 按钮形态可照抄「整栏宽 + 两行文字 + 等距竖排」，但配色用本项目 token。
- ✅ 字体：界面用系统无衬线，路径/偏移/日志用等宽。

## 必须复刻的交互细节
1. **拖拽**：把 exe 拖进窗口即填入 Source；拖入 `*.yml` 即载入补丁表。
2. 路径框右端的「清空」与「浏览」是两个独立小按钮，不合并。
3. 勾选状态实时反映到顶栏 `Selected Patches` 计数。
4. `Apply Patches` 在未选任何补丁时禁用；应用前若 Target 已存在，先确认覆盖。
5. 底栏语言切换只影响界面文案，不改变补丁表内容。
