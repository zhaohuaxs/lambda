//! Lambda 的协议：界面读的状态，以及改变状态的命令。
//!
//! 界面、Bern 生成的 CLI/MCP 都只通过这里访问业务层。
//!
//! 一个授权位置（SAF 树）是一条 `Location`，浏览到的每一级是 `entries`，
//! 导航路径是 `crumbs`。打开归档后，`archive_*` 这一组字段接管同一个页面：
//! 归档内的一层用 `archive_entries`，路径用 `archive_crumbs`。解压、密码簿
//! 与最近记录各有自己的状态字段，命令都追加在 M3 命令之后。
//!
//! 两条值与界面之间的约定：
//!
//! - `Entry` / `ArchiveEntry` **不带 URL**。URL 只有引擎需要（用来进目录 /
//!   读文件），把它放进 state 会让一万个条目多背 1.5 MB，而界面一个字都用
//!   不上。选中的是 `id`。
//! - 所有格式化在 Rust 里做完（`size_text` / `detail_text` / `sort_text`），
//!   UI 只显示字符串。
//! - 密码从不进 state：命令收到密码、用完即弃，密码簿只暴露标签与 id。

#[bern::protocol(namespace = "lambda", version = 1)]
pub mod lambda {
    use bern::prelude::*;

    /// 界面外观。`System` 跟随系统，`Light` / `Dark` 固定。
    #[value]
    pub enum Appearance {
        #[label(tr! { "zh-Hans" => "自动", "en" => "Auto" })]
        System,
        #[label(tr! { "zh-Hans" => "浅色", "en" => "Light" })]
        Light,
        #[label(tr! { "zh-Hans" => "深色", "en" => "Dark" })]
        Dark,
    }

    /// 强调色。`Brown` 与图标同色系，是默认；`Blue` 是原来的蓝色，
    /// `Red` 与它同明度、同饱和度。
    #[value]
    pub enum Accent {
        #[label(tr! { "zh-Hans" => "棕色", "en" => "Brown" })]
        Brown,
        #[label(tr! { "zh-Hans" => "蓝色", "en" => "Blue" })]
        Blue,
        #[label(tr! { "zh-Hans" => "红色", "en" => "Red" })]
        Red,
    }

    /// 一个用户授权过的位置（Android 是 SAF 树，Windows 是盘符下的目录）。
    #[value]
    pub struct Location {
        pub id: i64,
        #[content]
        pub label: String,
        /// 可用 / 授权已失效 / 已移除，界面直接显示。
        #[content]
        pub note: String,
        pub stale: bool,
        pub current: bool,
    }

    /// 当前目录里的一项。
    #[value]
    pub struct Entry {
        pub id: i64,
        #[content]
        pub name: String,
        /// 「1.2 MB」/「—」（文件夹没有大小）。
        #[content]
        pub size_text: String,
        /// 副标题：「文件夹 · 3 天前」。类型与时间合成一行，手机宽度下
        /// 三列放不下（2026-11-06 真机截图），时间就挪到名字下面。
        #[content]
        pub detail_text: String,
        /// 「✓」/「○」：每行那个选择按钮的标签。选中状态由引擎持有
        /// ——`Table.selection` 把 id 存在界面侧之后，换目录时旧 id 会残留，
        /// 表头的三态复选框就会在空表上显示「已全选」（2026-11-06 真机）。
        #[content]
        pub mark: String,
        pub folder: bool,
        pub archive: bool,
        pub chosen: bool,
    }

    /// 面包屑的一节；`id` 就是它在导航栈里的下标。
    #[value]
    pub struct Crumb {
        pub id: i64,
        #[content]
        pub name: String,
    }

    /// 归档里的一行。`id` 是完整路径的哈希，不是下标：排序、进出目录都不
    /// 改变它，选中集合因此可以只存 id。
    #[value]
    pub struct ArchiveEntry {
        pub id: i64,
        #[content]
        pub name: String,
        #[content]
        pub size_text: String,
        /// 「文件夹」/「加密文件」/「12.3 KB」这类副标题。
        #[content]
        pub detail_text: String,
        /// 「✓」/「○」。
        #[content]
        pub mark: String,
        pub folder: bool,
        pub chosen: bool,
        pub encrypted: bool,
        /// 已知的文本格式：大小列显示预览眼睛，而不是大小文字。
        pub preview: bool,
        /// 已知的图片格式：大小列显示图片预览按钮。
        pub image: bool,
    }

    /// 归档内的面包屑；`id` 是层级下标（0 是归档本身）。
    #[value]
    pub struct ArchiveCrumb {
        pub id: i64,
        #[content]
        pub name: String,
    }

    /// 首页的最近归档。只记位置与显示信息，不记密码。
    #[value]
    pub struct RecentArchive {
        pub id: i64,
        #[content]
        pub name: String,
        #[content]
        pub detail_text: String,
        #[content]
        pub size_text: String,
        /// 打开时需要密码：界面会先弹解锁框。
        pub encrypted: bool,
    }

    /// 密码簿的一条**元信息**。密文在系统凭据库里，state 里只有标签。
    #[value]
    pub struct SavedPassword {
        pub id: String,
        #[content]
        pub label: String,
        #[content]
        pub note: String,
    }

    #[state]
    pub struct State {
        /// 位置条。
        pub locations: Vec<Location>,
        /// 当前位置的条目，已按 `sort` 排好。
        pub entries: Vec<Entry>,
        /// 从位置根到当前目录。
        pub crumbs: Vec<Crumb>,
        /// 当前目录的名字，空态时显示位置名。
        #[content]
        pub path: String,
        /// 最近一次动作的结果 / 错误。
        #[content]
        pub status: String,
        /// 实际使用的引擎。
        #[content]
        pub engine: String,
        /// 界面外观：自动 / 浅色 / 深色。只是显示偏好，不涉及业务。
        pub appearance: Appearance,
        /// 强调色：棕色（默认）/ 蓝色 / 红色。
        pub accent: Accent,
        pub count: i64,
        pub selected: i64,
        /// 选中项的字节合计；CLI 用来核对，界面显示 `selected_text`。
        pub selected_bytes: i64,
        /// 「1.7 MB」：合计字节的人类可读形式。原始数字在真机上是
        /// 「1757282 字节」这种读不动的样式（2026-11-06 真机）。
        #[content]
        pub selected_text: String,
        /// 空目录时的提示；非空时为空串。
        #[content]
        pub empty_text: String,
        /// 排序状态：当前按哪个字段、是否降序。
        /// 「名称 ↑」/「修改时间 ↓」，直接显示。
        #[content]
        pub sort_text: String,
        /// 「全选」/「全不选」，随当前页的选中情况变。
        #[content]
        pub select_all_label: String,
        pub by_name: bool,
        pub by_size: bool,
        pub by_time: bool,
        pub descending: bool,
        pub has_locations: bool,
        pub has_entries: bool,
        pub can_go_up: bool,
        /// 还没有任何位置，界面显示引导卡片。
        pub needs_location: bool,
        pub busy: bool,

        // —— M3：压缩任务 ——
        /// 有任务在跑：界面显示进度区、禁用「压缩」。
        pub job_active: bool,
        /// 「压缩 3 项」（任务标题）。
        #[content]
        pub job_title: String,
        /// 「读取 2/3」「压缩中」「导出到位置」。
        #[content]
        pub job_phase: String,
        /// 当前文件名，进度区第二行。
        #[content]
        pub job_label: String,
        pub job_done: i64,
        pub job_total: i64,
        /// 0–100；`-1` 表示引擎还没报告过。
        pub job_percent: i64,
        /// 「45%」/「读取中」：`Progress` 要数字，文字另给一份。
        #[content]
        pub job_percent_text: String,
        /// Android 的引擎阶段无法中断，只能等当前 7zz 调用返回。
        pub job_cancellable: bool,

        // —— M3：压缩设置 ——
        /// 「7z」/「zip」。
        #[content]
        pub format_text: String,
        pub format_7z: bool,
        pub format_zip: bool,
        /// 「快速」/「标准」/「极限」。
        #[content]
        pub level_text: String,
        /// 0–9，传给 `-mx`。
        pub level: i64,
        /// 「压缩 3 项」/「先选要压缩的文件」。
        #[content]
        pub compress_label: String,
        /// 「输出到「lambda-m0」的根目录」：SAF 只能写树根。
        #[content]
        pub output_note: String,
        /// 「加密文件名」/「AES-256」/「不加密」。
        #[content]
        pub password_note: String,
        /// 有选中项且没有任务在跑：压缩按钮可点。
        pub can_compress: bool,

        // —— M3：上次没跑完的任务 ——
        /// 启动时读到一份没写完的任务记录（`Lambda-Product-Plan.md` 5.5）。
        pub has_leftover: bool,
        /// 「上次任务未完成：压缩 3 项（7z，停在「读取」）· 6 分钟前」。
        #[content]
        pub leftover_text: String,

        // —— M4：归档浏览 ——
        /// 当前打开着一个归档：Files 页改为显示归档内容。
        pub archive_open: bool,
        /// 归档需要密码但还没解开：界面据此弹解锁框。
        pub archive_locked: bool,
        /// 「「a.7z」已加密：输入密码，或从密码簿里选一个」。
        #[content]
        pub archive_lock_text: String,
        /// 归档文件名。
        #[content]
        pub archive_title: String,
        /// 归档内的路径，如「a.7z / sub / img」。
        #[content]
        pub archive_path: String,
        pub archive_crumbs: Vec<ArchiveCrumb>,
        pub archive_entries: Vec<ArchiveEntry>,
        pub archive_count: i64,
        pub archive_selected: i64,
        pub archive_selected_bytes: i64,
        /// 「1.7 MB」。
        #[content]
        pub archive_selected_text: String,
        /// 空层时的提示；非空时为空串。
        #[content]
        pub archive_empty_text: String,
        /// 「名称 ↑」。
        #[content]
        pub archive_sort_text: String,
        /// 「全选」/「全不选」。
        #[content]
        pub archive_select_all_label: String,
        pub archive_has_entries: bool,
        pub archive_can_go_up: bool,
        pub archive_by_name: bool,
        pub archive_by_size: bool,
        pub archive_descending: bool,
        /// 归档（或其任一条目）带加密标记；解压需要密码。
        pub archive_encrypted: bool,
        /// 「解压 2 项」/「先选要解压的项」。
        #[content]
        pub extract_label: String,
        pub can_extract: bool,
        /// 0 当前文件夹 / 1 智能解压 / 2 归档名文件夹 / 3 其他位置。
        pub extract_policy: i64,
        /// 「当前文件夹」「智能解压」这类当前策略名。
        #[content]
        pub policy_text: String,
        /// 「当前文件夹」只在位置根可用；浏览进子目录后为 false。
        pub policy_current_ok: bool,
        /// 「解压到「下载」的根目录」「还没有选择目标位置」。
        #[content]
        pub extract_target_text: String,
        pub extract_has_target: bool,

        // —— M4：最近打开 & 密码簿 ——
        pub recent: Vec<RecentArchive>,
        pub has_recent: bool,
        pub passwords: Vec<SavedPassword>,
        pub has_passwords: bool,

        // —— 文本预览 ——
        /// 预览模态打开中；背景整体禁用。
        pub preview_open: bool,
        /// 条目名。
        #[content]
        pub preview_title: String,
        /// 当前页文本；只放这一页，不把整份文件塞进状态。
        #[content]
        pub preview_text: String,
        /// 「第 2 / 7 页」；还没读到时是「读取中…」，读完前只报「第 2 页」。
        #[content]
        pub preview_page_text: String,
        /// 当前页号（1 起）；0 表示第一页还没解出来。
        pub preview_page: i64,
        /// 工作线程还在读。
        pub preview_loading: bool,
        pub preview_has_prev: bool,
        pub preview_has_next: bool,
        /// 编码 + 已读取 / 总大小 / 是否读完。
        #[content]
        pub preview_status: String,
        /// 「UTF-8」/「GB18030」/空串（还在检测）。
        #[content]
        pub preview_encoding: String,

        // —— 图片预览 ——
        /// 预览的是图片：界面显示 Image，而不是分页文本。
        pub preview_is_image: bool,
        /// 引擎提供的图片流地址；文本预览或未打开时为空。
        pub preview_image: MediaSource,
    }

    #[commands]
    pub trait Commands {
        // —— 位置 ——
        /// 把一个授权文件夹加入位置表；已存在则只更新标签。
        #[errors("location")]
        fn add_location(&mut self, tree: MediaSource) -> Result<(), CommandError>;

        /// 切换到某个位置的根。
        #[errors("location")]
        fn open_location(&mut self, id: i64) -> Result<(), CommandError>;

        /// 从位置表里删掉一个位置（不动磁盘上的文件）。
        #[errors("location")]
        fn remove_location(&mut self, id: i64) -> Result<(), CommandError>;

        /// 逐个位置重新列举，标记失效的。
        #[errors("location")]
        fn recheck_locations(&mut self) -> Result<(), CommandError>;

        // —— 浏览 ——
        /// 进入子目录（行双击 / 点击的主动作）。
        #[errors("browse")]
        fn open_dir(&mut self, id: i64) -> Result<(), CommandError>;

        /// 跳到面包屑上的某一级。
        #[errors("browse")]
        fn open_crumb(&mut self, id: i64) -> Result<(), CommandError>;

        /// 回到上一级。
        #[errors("browse")]
        fn go_up(&mut self) -> Result<(), CommandError>;

        /// 重新列举当前目录。
        #[errors("browse")]
        fn refresh(&mut self) -> Result<(), CommandError>;

        /// 按名称排序（再点一次反向）。
        fn sort_name(&mut self) -> Result<(), CommandError>;

        /// 按大小排序（再点一次反向）。
        fn sort_size(&mut self) -> Result<(), CommandError>;

        /// 按修改时间排序（再点一次反向）。
        fn sort_time(&mut self) -> Result<(), CommandError>;

        // —— 选择 ——
        /// 选中 / 取消选中一项。
        fn toggle(&mut self, id: i64) -> Result<(), CommandError>;

        /// 全选当前目录；已经全选时改为全不选。
        fn select_all(&mut self) -> Result<(), CommandError>;

        /// 行被点开：目录就进去，其余只报告（与选择互不影响）。
        #[errors("browse")]
        fn open(&mut self, id: i64) -> Result<(), CommandError>;

        // —— 诊断 ——
        /// 重新探测引擎。
        #[errors("engine")]
        fn probe(&mut self) -> Result<(), CommandError>;

        /// 清空状态里的结果（不清位置表）。
        #[idempotent]
        fn clear(&mut self) -> Result<(), CommandError>;

        /// 引擎边界补测：异常归档 / 路径穿越 / 耗时。界面不暴露，供 CLI 与
        /// 真机排查用（报告写在应用私有目录的 m5-report.txt）。
        #[errors("diagnostics")]
        fn boundary(&mut self) -> Result<(), CommandError>;

        // —— 压缩（M3）——
        /// 压缩格式换成 7z（可加密文件名）。
        fn use_7z(&mut self) -> Result<(), CommandError>;

        /// 压缩格式换成 zip（加密走 AES-256，文件名不可加密）。
        fn use_zip(&mut self) -> Result<(), CommandError>;

        /// 在 7z / zip 之间切换。
        fn toggle_format(&mut self) -> Result<(), CommandError>;

        /// 压缩级别：0 = 仅存储，1 = 快速，5 = 标准，9 = 极限。
        #[errors("options")]
        fn set_level(&mut self, level: i64) -> Result<(), CommandError>;

        /// 在「快速 → 标准 → 极限」之间循环。
        fn cycle_level(&mut self) -> Result<(), CommandError>;

        /// 把选中的项压成一个归档，导出到当前位置的根。
        ///
        /// 立刻返回：真正的工作在一个工作线程里，进度经 `wake` 搬进状态。
        /// 密码从界面传进来（它不住在状态里，CLI 也不该回显它）。
        #[errors("job")]
        fn compress(&mut self, password: String) -> Result<(), CommandError>;

        /// 请求取消当前任务。Android 的引擎阶段只能等 7zz 返回。
        #[errors("job")]
        fn cancel_job(&mut self) -> Result<(), CommandError>;

        /// 重跑上次没跑完的任务（记录里没有密码，要用户重新给）。
        #[errors("job")]
        fn retry_leftover(&mut self, password: String) -> Result<(), CommandError>;

        /// 丢掉上次没跑完的任务记录。
        #[idempotent]
        fn dismiss_leftover(&mut self) -> Result<(), CommandError>;

        // ---- 以下从 M4 追加；上面的顺序不能动（它是生成代码的调用下标）----

        // —— 归档浏览（M4）——
        /// 打开当前目录里的某个归档。`secret` 是密码簿条目的 id；非空时
        /// 优先用它，`password` 只是用户当场输入的备选。
        #[errors("archive")]
        fn open_archive(
            &mut self,
            id: i64,
            password: String,
            secret: String,
        ) -> Result<(), CommandError>;

        /// 从首页的最近记录重新打开。
        #[errors("archive")]
        fn open_recent(
            &mut self,
            id: i64,
            password: String,
            secret: String,
        ) -> Result<(), CommandError>;

        /// 给一个待解锁的归档再试一次密码。
        #[errors("archive")]
        fn unlock_archive(&mut self, password: String, secret: String) -> Result<(), CommandError>;

        /// 不打开这个加密归档了，清掉临时副本。
        #[idempotent]
        fn dismiss_unlock(&mut self) -> Result<(), CommandError>;

        /// 进入归档内的子文件夹。
        #[errors("archive")]
        fn archive_enter(&mut self, id: i64) -> Result<(), CommandError>;

        /// 回到归档内的上一级。
        #[errors("archive")]
        fn archive_up(&mut self) -> Result<(), CommandError>;

        /// 跳到归档面包屑的某一级。
        #[errors("archive")]
        fn open_archive_crumb(&mut self, id: i64) -> Result<(), CommandError>;

        /// 关闭归档，回到文件列表，并删掉临时副本。
        #[idempotent]
        fn close_archive(&mut self) -> Result<(), CommandError>;

        fn archive_toggle(&mut self, id: i64) -> Result<(), CommandError>;

        fn archive_select_all(&mut self) -> Result<(), CommandError>;

        fn archive_sort_name(&mut self) -> Result<(), CommandError>;

        fn archive_sort_size(&mut self) -> Result<(), CommandError>;

        // —— 解压（M4）——
        /// 解压策略：0 当前文件夹 / 1 智能 / 2 归档名文件夹 / 3 其他位置。
        #[errors("extract")]
        fn set_policy(&mut self, policy: i64) -> Result<(), CommandError>;

        /// 「其他位置」的目标树根。选中后策略自动切到「其他位置」。
        #[errors("extract")]
        fn set_target(&mut self, tree: MediaSource) -> Result<(), CommandError>;

        /// 开始解压。`all` 为 true 是当前层的全部，否则是勾选项。
        /// 密码的优先级：密码簿 > 本次输入 > 打开归档时的密码。
        #[errors("extract")]
        fn extract(
            &mut self,
            all: bool,
            password: String,
            secret: String,
        ) -> Result<(), CommandError>;

        // —— 压缩（M4：自定义归档名）——
        /// 和 `compress` 一样，只是归档名由用户给；`secret` 是密码簿 id。
        #[errors("job")]
        fn compress_named(
            &mut self,
            name: String,
            password: String,
            secret: String,
        ) -> Result<(), CommandError>;

        // —— 密码簿 & 最近记录 ——
        /// 保存一条密码：标签落 JSON，密文进系统凭据库。
        #[errors("password")]
        fn save_password(&mut self, label: String, password: String) -> Result<(), CommandError>;

        /// 忘掉一条保存的密码。
        #[errors("password")]
        fn forget_password(&mut self, id: String) -> Result<(), CommandError>;

        /// 清空最近打开列表（不动归档本身）。
        #[idempotent]
        fn clear_recent(&mut self) -> Result<(), CommandError>;

        // —— 归档浏览（M4：直接选文件）——
        /// 打开系统面板选中的归档文件，不要求先添加位置。加密归档会进入
        /// 「等密码」，由界面弹解锁框。
        #[errors("archive")]
        fn open_file(
            &mut self,
            file: MediaSource,
            password: String,
            secret: String,
        ) -> Result<(), CommandError>;

        // —— 外观 ——
        /// 切换界面外观：自动 / 浅色 / 深色；存在应用私有目录，下次启动沿用。
        fn set_appearance(&mut self, appearance: Appearance) -> Result<(), CommandError>;

        // ---- 以下从 M5 追加；上面的顺序不能动（它是生成代码的调用下标）----

        // —— 文本预览 ——
        /// 流式读取归档里的一个文本条目。密码复用打开归档时留在内存里的
        /// 那一份，不进入状态。
        #[errors("preview")]
        fn preview_open(&mut self, id: i64) -> Result<(), CommandError>;

        /// 关闭预览并停止后台读取。
        #[idempotent]
        fn preview_close(&mut self) -> Result<(), CommandError>;

        /// 翻页；`delta` 为 +1 / -1。下一页还没读出来时进入等待。
        fn preview_turn(&mut self, delta: i64) -> Result<(), CommandError>;

        // ---- 以下从 M6 追加；上面的顺序不能动 ----

        /// 切换强调色：棕色（默认）/ 蓝色 / 红色；存在应用私有目录，下次启动沿用。
        fn set_accent(&mut self, accent: Accent) -> Result<(), CommandError>;
    }
}
