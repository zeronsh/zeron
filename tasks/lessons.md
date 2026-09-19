# Lessons

## 验证方法

- **PowerShell 管道会按控制台编码解码原生命令输出**。`git show <path> | Select-String`、
  `| Out-File` 在未设置编码时把 UTF-8 源码里的非 ASCII 字符解释成乱码（实测 `…` 变成
  `U+9225` + `?`），逐字对比文案时会误报“文案不一致”。核对 HEAD 内容前先设置：

  ```powershell
  [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
  git show "HEAD:<path>" | Out-File -FilePath $tmp -Encoding utf8
  $text = Get-Content $tmp -Raw -Encoding utf8
  ```

  读工作区文件直接用 `Get-Content <file> -Raw -Encoding utf8`，不经过管道。

- **`.NET` 正则默认不开启多行模式**。用 `[regex]::Matches($t, '^\s{4}Name = ...')` 这类带
  `^` 的行首锚点扫描表格时，必须显式写 `(?m)`（跨行匹配再叠加 `(?s)`），否则 `^` 只匹配整段
  文本开头，结果直接是 0 命中，容易被误判成“表里没有这些行”。另外正则里的分组编号要小心：
  `A(?:B|C)` 用非捕获组，否则 `$m.Groups[1]` 会变成外层前缀而不是你要抓的名字。

- **按行扫描字面量会漏掉跨行字符串**。源码里用行尾 `\` 续行的长文案、以及 `format!`
  位置参数（`{}` 或 `{name}`）都不匹配“单行里含引号文本”的模式。核对时先合并续行、折叠空白，
  再把 `{name}` 归一到 `{}` 与 `format!` 原文比较；同时提醒执行人自行 grep 复核，别只依赖清单。

- **文案清单要按"渲染路径"而不是"字符串形状"来扫**。只扫 `"..."` 或以大写开头的完整句子，
  会漏掉 `aria_label("...")`、`child(SharedString::from("..."))`、`format!("{} tokens used…")`
  这类以占位符开头的模板，也会把 `decode_*` 返回的 `String` 误判成"协议载荷"。判定的依据是
  “谁消费这个字符串、是否落到屏幕上”：仓库自有文案要迁，引擎/序列化/日志/文件名数据保留英文。
  一个文件"迁移完成"的结论必须来自逐条消费链核对，而不是文件是否出现在批次清单里。

- **全仓机械扫描用来找"漏网文案"，分类仍要逐条走消费链**。提取规则（≥6 字符、同时含空格与字母
  的字面量，排除 `i18n.rs` 与每个文件首个 `#[cfg(test)]` 之后的内容）在 `crates/ui/src` 约 50 个
  文件里给出 377 个候选，交给只读子代理按"是否落到屏幕"分类，再由主 Agent 逐条读代码确认。
  子代理会把已经丢弃的错误、只进文件名或提示词载荷的字符串报成缺口，所以分类结果只是待核对
  清单，不是结论。扫描范围按 crate 划定：只扫 `crates/ui/src` 时，`crates/theme` 自有的
  `unknown custom theme`、条目/变体/字节上限等消息自然落在清单外，这是刻意的边界，要在计划里
  写成"未迁移"而不是顺手一起改。

- **替换片段别丢行尾换行**。`search_replace` 的 `old_string` 以 `},\n` 结尾而 `new_string` 不以
  换行结尾时，匹配到的换行会被一起吃掉，下一行被拼到同一行（实测把一条消息行与下一个 `{` 合并，
  编译期才暴露）。多行替换让两端保持同样的行尾结构，改完立即 `read_file` 复核边界行。

## 编译与平台

- **共享类型的形状变更会静默破坏 `cfg` 屏蔽的平台模块**。把 `CaptureError::CaptureFailed(String)`
  改成结构化载体后，`appshots/macos.rs` 与 `appshots/linux/*` 里的旧构造点全部失效，而本机
  （Windows）`cargo check` 因 `#[cfg(target_os = ...)]` 完全看不到它们，检查全绿却已经破坏
  macOS/Linux 构建。改公开枚举/结构体形状前，先 `grep` 该类型在 `#[cfg]` 模块里的构造点，
  把它们一并纳入同一批改动；平台文件只能用 `rustfmt`（语法级）+ 逐行读签名与调用点 + 与 HEAD
  的逐字英文比对来验证，并在交付时明确说明"本机未编译验证，需在 macOS/Linux 各跑一次"。

- **删除依赖声明前先全目录 grep，而不是只查 `src/**/*.rs`**。`zeron-ui` 最后一个 `thiserror`
  使用点消失后，依赖仍留在 `Cargo.toml`；确认无引用（含被 `cfg` 屏蔽的文件）再删除，并核对
  `Cargo.lock` 只少了该 crate 依赖列表里的一行。

- **`rustfmt` 的 diff 可能远大于实际改动：改到稠密表达式里的一行，会重排整条语句**。
  `browser/linux/mod.rs` 的帧读取线程、`browser/view.rs` 末尾的 `div()` 链都是"压行"写法的
  单个表达式，改动其中一处字符串后 `rustfmt` 会把整段展开（linux 那次约 90 行、view 约 40 行），
  看起来像无关 churn，实际是格式化的必然结果。判断方法：把 `HEAD` 版本导出到临时文件（用
  `cmd /c "git cat-file blob HEAD:<path> > file"` 保留原始字节，`git show | Out-File` 会带 BOM
  影响 `rustfmt`）后跑 `rustfmt --check`，能区分"文件本来就脏"和"我的改动让格式化结果变了"。
  交付时说明这部分是空白差异，避免审查者以为改了逻辑。

- **用临时文件核对 `rustfmt` 状态时，`mod` 声明会让 `rustfmt` 直接报错退出**。把 `HEAD` 的
  `shell.rs` 导出成 `%TEMP%\crates_ui_src_shell.rs` 再 `rustfmt --check`，它会去找同目录下的
  `command_palette.rs`，报 `failed to resolve mod` 并以退出码 1 结束；如果同时过滤输出（只留
  `Diff in` 行），就会看到一个"空输出 + 退出码 1"的假象，误判成"文件本来就干净"。加
  `--config skip_children=true` 才能按单文件检查，或者先确认退出码 0 再下结论。

- **平台专属的"死文案"要按是否真的会渲染来判定**。`browser/linux/helper.c` 自建了 8 条上下文
  菜单标签，但 `linux/mod.rs::menu_label` 按 action id 覆盖了它们，实际不会显示；同一文件
  `terminated()` 里那句则确实会被渲染（经 helper 的 `error` 字段落到 `Detail`）。前者是
  fallback，后者才是缺口 —— 结论必须来自"谁最后渲染这条字符串"，而不是"文件里是否还有英文"。

## 跨 crate 共享文案

- **分类留在下层、文案搬到上层，比给下层塞 locale 更省事**。`crates/proto/view.rs` 里的工具
  chip 标签、分组摘要、紧凑相对时间，`crates/ui` 和终端视图都要用；proto 不依赖 UI、也不该
  为了翻译反向依赖，于是 proto 保留原有英文渲染函数（签名不变、输出逐字不变），只把"这是哪种
  chip / 哪个时间桶 / 摘要由哪些片段组成"抽成结构化枚举（`ToolChip`、`ToolChipDetail`、
  `ToolSummarySegment`、`CompactAge`），UI 侧用这些枚举查自己的消息表。这样上层只重复了"文案"，
  不会出现两份分类逻辑。

- **重复的英文要变成有断言的约束，否则迟早跑偏**。搬完文案后必然存在"proto 的英文渲染"和
  "UI 表的英文行"两份同样的字面量。加一条测试直接断言两者对同一输入输出相等
  （遍历所有枚举分支各断言一次），比人工比对更持久；同时保留原有的英文断言测试不动，它顺带
  证明了 proto 的英文输出没有因为重构而变化。

- **locale 无关的指纹必须只哈希稳定标识**。工具行的重排判定原本哈希英文标签字节
  （`tool_fingerprint`）。文案改由 locale 决定后，若继续哈希渲染结果，切换语言会让所有工具行
  重新拼接。改成哈希枚举 kind + proto 自己的英文明细长度即可，语义不变且与语言无关。

- **共享摘要的"拼接与首字母大写"也属于共享格式**。把每个片段改成结构化枚举后，"以 ` · ` 连接
  并把首段首字母大写"这段逻辑若在 UI 里重写一遍就多了一份格式实现；把它作为
  `summary_line(segments)` 暴露出来，两边都调用，中文无大小写所以天然不受影响。

- **枚举的"兜底分支"可能是上层不可达的，测试要写在片段层**。摘要里的"裸工具计数"只有在空集合
  时才会生成，而 UI 包装函数对空集合直接短路返回空串，因此这一分支在 UI 里永远走不到。与其在
  集成测试里伪造一个到不了的场景，不如直接对该片段（`summary_segment(...)`）断言中英文，并在
  注释里说明可达性。

## 文案归属与就地解析

- **能在生成处拿到 `App` 的文案就在生成处解析 locale**，不要为它引入结构化错误类型 + 渲染期
  查表。`theme_library.rs` 的失败、`settings.rs::install_new_thread_composer_background` 都是
  `fn(..., cx)`，直接 `let locale = i18n::locale(cx);` 起头、`anyhow!(i18n::translate(...))` 收尾
  即可。代价是这条已存储的提示在切换语言后保持旧语言，直到页面重新渲染 —— 与 `background_error`、
  `library_error` 那类 `Option<SharedString>` 字段的限制相同（计划文档的 "Known limitations" 有
  记录）。反过来的情形同样明确：行缓存（transcript 工具行）和持久化字段必须保持 locale 无关。

- **下层 crate 自有的文案不要在同一轮里连带重构**。把 `crates/ui` 自有的四条主题库失败本地化后，
  仍然显示英文的是 `crates/theme` 写的那几条（`unknown custom theme`、条目/变体/字节上限、库
  文件读取失败），它们直接出现在导入对话框与主题库条里。要动就得先在 `zeron-theme` 里做结构化
  错误（照 `crates/proto/view.rs` 那套"分类在下、文案在上"的分层）再在上层映射，属于独立一步；
  同一轮里把它顺手改掉会变成跨 crate 的 API 变更。

- **同一句话在两处产出就要收敛到一个实现**。评论徽章原本有 `comments::chip_label(count)` 与
  `badges` 里的徽章渲染两条路，工具片卡的 `workspace` / `{pattern} in {path}` 也在 chip 头部与
  展开块各拼一次。这次分别收敛为 `BadgeLabel::Comments(usize)` + `text(locale)` 与
  `chip_detail_text(detail, locale)`（`tool_chip_text` 与 `CallBlock::resolved` 共用），
  删掉旧函数并迁移其测试；否则同一句话的两种译法迟早分叉。

- **下层 crate 的错误类型要"可翻译"而不是"带语言"**。`zeron-theme::LibraryError` 七个变体
  保留原有的英文 `Display`（`english()` 逐字不变，因为其中一部分会写进
  `CustomThemeStatus::Warning` 持久化到库里，CLI 与 tracing 也读同一份文本），上层用
  `error.downcast_ref::<LibraryError>()` 把它映射成自己的消息行；诊断类消息（含路径、解析器
  载荷）不做变体，按原样透出。测试把每个变体的英文与上层 `Locale::En` 的输出钉在一起 ——
  跨 crate 的重复英文因此变成断言，而不是两处各自维护的文案。

- **迁移期的保守默认值要写明翻转条件**。`LanguagePreference::default()` 在文案迁移期间写死
  `English`，代码注释与计划文档都注明"最后一个阶段翻成 `System`"；条件满足后翻转它需要同步
  四处：代码注释、计划文档的假设与收尾结论、验收清单、以及钉住旧默认值的测试（改名为
  `default_follows_the_system_language` 并同时断言 `System` 与三个 `resolve` 结果，避免"默认值"
  与"回退路径"被混为一谈）。
