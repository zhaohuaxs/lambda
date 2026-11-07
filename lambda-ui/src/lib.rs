//! Lambda 的界面：首页、文件、任务、密码簿、设置五个页面，外加压缩、解压、
//! 解锁三个模态。
//!
//! 宽窗口是侧栏 + 内容，手机是底部标签栏；同一份源码生成 Windows 与 Android
//! 两个前端。所有固定的句子走 `tr!`，文件相关的内容由引擎格式化好后直接显示。
//!
//! 几条不能走岔的约定：
//!
//! - 密码不进入协议状态：界面把它放在会话草稿里，命令收到即用；密码簿只
//!   暴露标签，密文在系统凭据库。关闭模态时草稿里的密码会被清掉。
//! - 选中状态在引擎里：每行渲染一个选择图标按钮，`Table.selection` 不用。
//! - 表格只有三列：选 / 名称（副标题）/ 大小（目录与归档显示强调色箭头）。四列时
//!   名称列被挤到截断（2026-11-06 真机）。
//! - 失效目录换成「重新授权」按钮，选择器交回的树走 `add_location`。
//! - Android 的 SAF 不能删除、重命名、移动用户文件；界面只承诺写到目录根，
//!   同名自动改名。

use bern::prelude::*;
use lambda_protocol::lambda;

style! {
    accents {
        brown: "#7A4606",
        blue: "#2F5BFF",
        red: "#FF2F2F",
    }

    Stack.card {
        background: fill.subtle,
        corner: 12,
    }

    // 首页最近记录、目录列表这类整行可点的条目。
    Button.row {
        corner: 8,
        hover: background(fill.subtle),
        pressed: scale(0.99, press),
        settle: snappy,
    }

    // 表格里每行的选择格：整格可点，悬停时给一点底色。
    Button.rowicon {
        hover: background(fill.subtle),
        pressed: scale(0.98, press),
        settle: snappy,
    }

    // 表格行尾的进入箭头是这一行的主动作，用强调色点出来。
    Button.rowarrow {
        foreground: accent,
        hover: background(fill.subtle),
        pressed: scale(0.98, press),
        settle: snappy,
    }
}

#[bern::value]
pub enum Page {
    #[label(tr! { "zh-Hans" => "首页", "en" => "Home" })]
    Home,
    #[label(tr! { "zh-Hans" => "文件", "en" => "Files" })]
    Files,
    #[label(tr! { "zh-Hans" => "任务队列", "en" => "Tasks" })]
    Tasks,
    #[label(tr! { "zh-Hans" => "密码簿", "en" => "Passwords" })]
    Passwords,
    #[label(tr! { "zh-Hans" => "设置", "en" => "Settings" })]
    Settings,
}

/// 界面这一侧的一次性输入。真正的状态都在引擎里，这里只放选择器与密码。
#[bern::session]
pub struct Draft {
    pub page: Page,
    /// 首页「打开归档…」与文件页的归档选择器。
    pub picked: MediaSource,
    /// 添加目录 / 重新授权。
    pub tree: MediaSource,
    /// 解压到「其他目录」时选的目标树。
    pub target_tree: MediaSource,

    pub compress_open: bool,
    pub compress_name: String,
    pub compress_password: String,
    /// 密码簿条目的 id；非空时优先于手输密码。
    pub compress_secret: String,

    pub extract_open: bool,
    pub extract_password: String,
    pub extract_secret: String,

    pub unlock_password: String,
    pub unlock_secret: String,

    pub password_label: String,
    pub password_value: String,

    pub retry_password: String,
}

#[component]
pub fn app(app: lambda::Client, windows: AppWindows, #[session] draft: Draft) -> impl View {
    ui! {
        WindowSet(handle: AppWindows, primary: main, accent: app.accent, appearance: app.appearance) {
            Window(
                id: main,
                title: tr! { "zh-Hans" => "Lambda · 压缩包管理器", "en" => "Lambda · Archives" },
                initial: open,
                chrome: unified,
                width: 1040,
                height: 780,
                min_width: 680,
                min_height: 540
            ) {
                Stack {
                    // 有模态时背景整体禁用：遮罩下面的按钮不该还能点。
                    Stack(enabled: !(draft.compress_open || draft.extract_open || app.archive_locked || app.preview_open)) {
                        Adaptive(id: shell) {
                            Layout(size: compact) { phone_shell(app: app) }
                            // 手机横过来：够宽但没有高度，仍用标签栏。
                            Layout(size: expanded) {
                                if window.short {
                                    phone_shell(app: app)
                                } else {
                                    desktop_shell(app: app)
                                }
                            }
                        }
                    }
                    if draft.compress_open {
                        compress_dialog(app: app)
                    }
                    if draft.extract_open {
                        extract_dialog(app: app)
                    }
                    if app.archive_locked {
                        unlock_dialog(app: app)
                    }
                    if app.preview_open {
                        preview_dialog(app: app)
                    }

                    // 成功打开归档（包括解锁后）就翻到文件页。
                    if app.archive_open {
                        Watch(value: app.archive_open) => draft.page.set(Page::Files)
                    }
                    // 任务一起来，两个开始对话框就收起；密码同时从草稿里清掉。
                    Watch(value: app.job_active) => draft.compress_open.set(false)
                    Watch(value: app.job_active) => draft.extract_open.set(false)
                    Watch(value: draft.compress_open) => draft.compress_password.set("")
                    Watch(value: draft.compress_open) => draft.compress_secret.set("")
                    Watch(value: draft.extract_open) => draft.extract_password.set("")
                    Watch(value: draft.extract_open) => draft.extract_secret.set("")
                    Watch(value: app.archive_locked) => draft.unlock_password.set("")
                    Watch(value: app.archive_locked) => draft.unlock_secret.set("")
                    Watch(value: app.job_active) => draft.retry_password.set("")
                }
            }
        }
    }
}

/// 宽窗口：左侧固定的页面栏，右侧内容。
#[component]
pub fn desktop_shell(app: lambda::Client, #[session] draft: Draft) -> impl View {
    ui! {
        SplitView {
            Sidebar(id: pages, selection: draft.page, fallback: Page::Home, width: 232) {
                SidebarHeader(role: title) {
                    Row(gap: 10, padding_x: 16, padding_y: 14, align: center) {
                        Label(
                            title: tr! { "zh-Hans" => "Lambda", "en" => "Lambda" },
                            icon: "archivebox",
                            display: icon,
                            font_size: 24
                        )
                        Column(gap: 1, align: leading) {
                            Text(
                                content: tr! { "zh-Hans" => "Lambda", "en" => "Lambda" },
                                style: headline,
                                bold: true
                            )
                            Text(
                                content: tr! { "zh-Hans" => "7-Zip 压缩包管理器", "en" => "7-Zip archive manager" },
                                style: caption2,
                                emphasis: secondary,
                                lines: 1
                            )
                        }
                    }
                }
                SidebarItem(value: Page::Home, label: tr! { "zh-Hans" => "首页", "en" => "Home" }, icon: "house")
                SidebarItem(value: Page::Files, label: tr! { "zh-Hans" => "文件", "en" => "Files" }, icon: "archivebox")
                SidebarItem(value: Page::Tasks, label: tr! { "zh-Hans" => "任务队列", "en" => "Tasks" }, icon: "arrow.down.circle")
                SidebarItem(value: Page::Passwords, label: tr! { "zh-Hans" => "密码簿", "en" => "Passwords" }, icon: "shield")
                SidebarItem(value: Page::Settings, label: tr! { "zh-Hans" => "设置", "en" => "Settings" }, icon: "gearshape")
                SidebarFooter {
                    Column(gap: 3, padding_x: 16, padding_y: 12, align: leading) {
                        if app.job_active {
                            Text(content: app.job_title, style: caption, bold: true, lines: 1)
                            Text(content: app.job_phase, style: caption2, emphasis: secondary, lines: 1)
                        } else if app.has_leftover {
                            Text(content: tr! { "zh-Hans" => "上次任务未完成", "en" => "A task did not finish" }, style: caption, color: status.warning, lines: 1)
                        } else {
                            Text(content: app.engine, style: caption2, emphasis: secondary, lines: 1)
                        }
                        Text(
                            content: tr! { "zh-Hans" => "版本 {version}", "en" => "Version {version}"; version: window.app_version },
                            style: caption2,
                            emphasis: secondary,
                            lines: 1
                        )
                    }
                }
            }
            Detail(titled: false) {
                page_view(app: app)
            }
        }
    }
}

/// 手机：五个标签页，浮动标签栏。有任务或未完成任务时，标签栏上方浮一条
/// 任务配件，点「查看」去任务页。
#[component]
pub fn phone_shell(app: lambda::Client, #[session] draft: Draft) -> impl View {
    ui! {
        Tabs(id: phone_tabs, selection: draft.page, fallback: Page::Home, bar: floating) {
            Tab(value: Page::Home, label: tr! { "zh-Hans" => "首页", "en" => "Home" }, icon: "house") {
                page_view(app: app)
            }
            Tab(value: Page::Files, label: tr! { "zh-Hans" => "文件", "en" => "Files" }, icon: "archivebox") {
                page_view(app: app)
            }
            Tab(value: Page::Tasks, label: tr! { "zh-Hans" => "任务", "en" => "Tasks" }, icon: "arrow.down.circle") {
                page_view(app: app)
            }
            Tab(value: Page::Passwords, label: tr! { "zh-Hans" => "密码", "en" => "Passwords" }, icon: "shield") {
                page_view(app: app)
            }
            Tab(value: Page::Settings, label: tr! { "zh-Hans" => "设置", "en" => "Settings" }, icon: "gearshape") {
                page_view(app: app)
            }
            TabAccessory(shown: app.job_active || app.has_leftover) {
                task_accessory(app: app)
            }
        }
    }
}

#[component]
pub fn task_accessory(app: lambda::Client, #[session] draft: Draft) -> impl View {
    ui! {
        Stack(variant: card) {
            Row(gap: 10, padding_x: 12, padding_y: 8, align: center) {
                Label(
                    title: tr! { "zh-Hans" => "任务", "en" => "Task" },
                    icon: "arrow.down.circle",
                    display: icon,
                    font_size: 16
                )
                Column(gap: 1, align: leading) {
                    if app.job_active {
                        Text(content: app.job_title, style: caption, bold: true, lines: 1)
                        Text(content: app.job_phase, style: caption2, emphasis: secondary, lines: 1)
                    } else {
                        Text(content: tr! { "zh-Hans" => "上次任务未完成", "en" => "A task did not finish" }, style: caption, bold: true, lines: 1)
                        Text(content: tr! { "zh-Hans" => "去任务页查看", "en" => "Open Tasks" }, style: caption2, emphasis: secondary, lines: 1)
                    }
                }
                Spacer
                if app.job_active {
                    Text(content: app.job_percent_text, style: caption)
                }
                Button(
                    id: accessory_view,
                    label: tr! { "zh-Hans" => "查看", "en" => "View" },
                    shape: chip,
                    prominence: plain,
                    height: 28
                ) => draft.page.set(Page::Tasks)
            }
        }
    }
}

#[component]
pub fn page_view(app: lambda::Client, #[session] draft: Draft) -> impl View {
    ui! {
        match draft.page {
            Page::Home => { home_page(app: app) }
            Page::Files => { files_page(app: app) }
            Page::Tasks => { tasks_page(app: app) }
            Page::Passwords => { passwords_page(app: app) }
            Page::Settings => { settings_page(app: app) }
        }
    }
}

#[component]
pub fn home_page(app: lambda::Client, #[session] draft: Draft) -> impl View {
    ui! {
        ScrollView {
            Column(gap: 16, padding: 18) {
                Stack(variant: card) {
                    Column(gap: 12, padding: 20, align: leading) {
                        Row(gap: 10, align: center) {
                            Label(
                                title: tr! { "zh-Hans" => "Lambda", "en" => "Lambda" },
                                icon: "archivebox",
                                display: icon,
                                font_size: 26
                            )
                            Text(
                                content: tr! { "zh-Hans" => "Lambda", "en" => "Lambda" },
                                style: large_title,
                                bold: true
                            )
                        }
                        Text(
                            content: tr! { "zh-Hans" => "7-Zip 压缩包管理器", "en" => "7-Zip archive manager" },
                            style: title3,
                            bold: true
                        )
                        Text(
                            content: tr! {
                                "zh-Hans" => "打开、浏览、创建与解压压缩包。密码只在命令执行时交给 7-Zip；保存的密码进入系统凭据库，界面只显示标签。",
                                "en" => "Open, browse, create and extract archives. Passwords reach 7-Zip only for the command itself; saved passwords live in the system credential store and only labels are shown."
                            },
                            style: body,
                            emphasis: secondary,
                            lines: 4
                        )
                        Flow(gap: 8) {
                            Button(
                                id: home_open_archive,
                                label: tr! { "zh-Hans" => "打开归档…", "en" => "Open Archive…" },
                                prominence: prominent,
                                shape: capsule,
                                height: 40,
                                pick: draft.picked,
                                files: "any"
                            ) => app.open_file(draft.picked, "", "")
                            Button(
                                id: home_browse,
                                label: tr! { "zh-Hans" => "浏览文件", "en" => "Browse Files" },
                                shape: capsule,
                                height: 40
                            ) => draft.page.set(Page::Files)
                            Button(
                                id: home_add_location,
                                label: tr! { "zh-Hans" => "添加目录", "en" => "Add a Directory" },
                                shape: capsule,
                                height: 40,
                                pick: draft.tree,
                                files: "folder"
                            ) => app.add_location(draft.tree)
                        }
                    }
                }

                if app.job_active {
                    Stack(variant: card) {
                        Column(gap: 8, padding: 16, align: leading) {
                            Row(gap: 8, align: center) {
                                Text(content: app.job_title, style: headline, bold: true, lines: 1)
                                Spacer
                                Text(content: app.job_percent_text, style: caption)
                            }
                            Text(content: app.job_phase, style: caption, emphasis: secondary, lines: 1)
                            Button(
                                id: home_task_view,
                                label: tr! { "zh-Hans" => "查看任务", "en" => "View Task" },
                                shape: chip,
                                prominence: plain,
                                height: 30
                            ) => draft.page.set(Page::Tasks)
                        }
                    }
                }

                if app.has_locations {
                    Column(gap: 8, align: leading) {
                        Text(content: tr! { "zh-Hans" => "目录", "en" => "Directories" }, style: title3, bold: true)
                        Flow(gap: 6) {
                            ForEach(items: app.locations, key: id) |location| {
                                if location.stale {
                                    Button(
                                        id: home_reauth,
                                        label: tr! {
                                            "zh-Hans" => "↻ {name}", "en" => "↻ {name}";
                                            name: location.label
                                        },
                                        shape: chip,
                                        prominence: plain,
                                        pick: draft.tree,
                                        files: "folder"
                                    ) => app.add_location(draft.tree)
                                } else {
                                    Button(
                                        id: home_location,
                                        label: location.label,
                                        shape: chip,
                                        prominence: plain
                                    ) => app.open_location(location.id)
                                }
                            }
                        }
                    }
                } else {
                    Text(
                        content: tr! {
                            "zh-Hans" => "还没有目录：添加一个文件夹，或直接打开一个归档。",
                            "en" => "No directories yet: add a folder, or open an archive directly."
                        },
                        style: caption,
                        emphasis: secondary,
                        lines: 2
                    )
                }

                if app.has_recent {
                    Column(gap: 8, align: leading) {
                        Row(gap: 8, align: center) {
                            Text(content: tr! { "zh-Hans" => "最近打开", "en" => "Recently Opened" }, style: title3, bold: true)
                            Spacer
                            Button(
                                id: home_clear_recent,
                                label: tr! { "zh-Hans" => "清空", "en" => "Clear" },
                                prominence: plain,
                                height: 28
                            ) => app.clear_recent()
                        }
                        Stack(variant: card) {
                            Column(gap: 0) {
                                ForEach(items: app.recent, key: id) |recent| {
                                    Button(id: home_recent, label: recent.name, variant: row) {
                                        Row(gap: 10, padding_x: 14, padding_y: 10, align: center) {
                                            Column(gap: 2, align: leading) {
                                                Text(content: recent.name, lines: 1)
                                                Text(content: recent.detail_text, style: caption, emphasis: secondary, lines: 1)
                                            }
                                            Spacer
                                            if recent.encrypted {
                                                Label(
                                                    title: tr! { "zh-Hans" => "加密", "en" => "Encrypted" },
                                                    icon: "shield",
                                                    display: icon,
                                                    font_size: 12
                                                )
                                            }
                                            Text(content: recent.size_text, style: caption, emphasis: secondary)
                                        }
                                    } => app.open_recent(recent.id, "", "")
                                }
                            }
                        }
                    }
                } else {
                    Text(
                        content: tr! { "zh-Hans" => "打开过的归档会出现在这里。", "en" => "Archives you open show up here." },
                        style: caption,
                        emphasis: secondary
                    )
                }

                Stack(variant: card) {
                    Column(gap: 8, padding: 16, align: leading) {
                        Text(content: tr! { "zh-Hans" => "密码与隐私", "en" => "Passwords and Privacy" }, style: headline, bold: true)
                        Text(
                            content: tr! {
                                "zh-Hans" => "密码住在本应用的会话草稿里，命令执行时经参数交给 7-Zip；协议状态、日志与任务记录都不含密码。密码簿的密文在系统凭据库，状态里只有标签。",
                                "en" => "Passwords stay in this session's draft and reach 7-Zip as command arguments. Protocol state, logs and task records never contain them. The password vault keeps ciphertext in the system credential store; state holds labels only."
                            },
                            style: caption,
                            emphasis: secondary,
                            lines: 5
                        )
                        Text(
                            content: tr! {
                                "zh-Hans" => "Android 的 SAF 不允许删除、重命名或移动你的文件；压缩与解压结果写到所选目录的根目录，同名文件会自动改名。",
                                "en" => "Android's SAF does not let an app delete, rename or move your files; archives and extracted files go to the root of the chosen directory, and name clashes are renamed."
                            },
                            style: caption,
                            emphasis: secondary,
                            lines: 4
                        )
                    }
                }

                if app.status != "" {
                    Text(content: app.status, style: caption, emphasis: secondary, lines: 2)
                }
            }
        }
    }
}

#[component]
pub fn files_page(app: lambda::Client, #[session] draft: Draft) -> impl View {
    ui! {
        if app.archive_open {
            archive_page(app: app)
        } else {
            Column(gap: 4) {
                Column(gap: 8, padding_x: 16, padding_y: 6) {
                    Row(gap: 8, align: center) {
                        Column(gap: 2, align: leading) {
                            Text(content: tr! { "zh-Hans" => "目录与文件", "en" => "Directories and Files" }, style: title2, bold: true)
                            Text(content: app.path, style: caption, emphasis: secondary, lines: 1)
                        }
                        Spacer
                    }
                    Flow(gap: 8) {
                        Button(
                            id: files_open_pick,
                            label: tr! { "zh-Hans" => "打开归档…", "en" => "Open Archive…" },
                            prominence: prominent,
                            shape: capsule,
                            height: 36,
                            pick: draft.picked,
                            files: "any"
                        ) => app.open_file(draft.picked, "", "")
                        Button(
                            id: files_new_archive,
                            label: tr! {
                                "zh-Hans" => "压缩选中的 {count} 项",
                                "en" => "Compress {count} Selected";
                                count: app.selected
                            },
                            shape: capsule,
                            height: 36,
                            enabled: app.selected > 0
                        ) => draft.compress_open.set(true)
                    }
                    Flow(gap: 6) {
                        ForEach(items: app.locations, key: id) |location| {
                            if location.stale {
                                Button(
                                    id: files_reauth,
                                    label: tr! {
                                        "zh-Hans" => "↻ {name}", "en" => "↻ {name}";
                                        name: location.label
                                    },
                                    shape: chip,
                                    prominence: plain,
                                    pick: draft.tree,
                                    files: "folder"
                                ) => app.add_location(draft.tree)
                            } else {
                                Button(
                                    id: files_location,
                                    label: location.label,
                                    shape: chip,
                                    prominence: plain
                                ) => app.open_location(location.id)
                            }
                        }
                        Button(
                            id: files_add_location,
                            label: tr! { "zh-Hans" => "＋ 目录", "en" => "＋ Directory" },
                            shape: chip,
                            pick: draft.tree,
                            files: "folder"
                        ) => app.add_location(draft.tree)
                    }
                    if app.needs_location {
                        Text(
                            content: tr! {
                                "zh-Hans" => "还没有目录：点「＋ 目录」选一个文件夹，或用「打开归档…」直接选文件。",
                                "en" => "No directory yet: use “＋ Directory” to pick a folder, or “Open Archive…” to pick a file."
                            },
                            style: caption,
                            emphasis: secondary,
                            lines: 2
                        )
                        Text(
                            content: tr! {
                                "zh-Hans" => "Android 不允许把存储根或 Download 根授权给应用，请进入子文件夹再点「使用此文件夹」。",
                                "en" => "Android refuses the storage or Download root: open a subfolder first, then “Use this folder”."
                            },
                            style: caption,
                            emphasis: secondary,
                            lines: 2
                        )
                    }
                    Flow(gap: 4) {
                        ForEach(items: app.crumbs, key: id) |crumb| {
                            Button(
                                id: files_crumb,
                                label: crumb.name,
                                height: 30
                            ) {
                                Row(gap: 0, padding_x: 10, height: 30, align: center) {
                                    Text(content: crumb.name, style: control, color: accent, lines: 1)
                                }
                            } => app.open_crumb(crumb.id)
                        }
                        Button(
                            id: files_up,
                            label: tr! { "zh-Hans" => "↑ 上级", "en" => "↑ Up" },
                            prominence: plain,
                            height: 30,
                            enabled: app.can_go_up
                        ) => app.go_up()
                        Button(
                            id: files_select_all,
                            label: app.select_all_label,
                            prominence: plain,
                            height: 30,
                            enabled: app.has_entries
                        ) => app.select_all()
                        Button(
                            id: files_sort_name,
                            label: tr! { "zh-Hans" => "名称", "en" => "Name" },
                            prominence: plain,
                            height: 30
                        ) => app.sort_name()
                        Button(
                            id: files_sort_size,
                            label: tr! { "zh-Hans" => "大小", "en" => "Size" },
                            prominence: plain,
                            height: 30
                        ) => app.sort_size()
                        Button(
                            id: files_sort_time,
                            label: tr! { "zh-Hans" => "时间", "en" => "Time" },
                            prominence: plain,
                            height: 30
                        ) => app.sort_time()
                        Button(
                            id: files_refresh,
                            label: tr! { "zh-Hans" => "刷新", "en" => "Refresh" },
                            prominence: plain,
                            height: 30
                        ) => app.refresh()
                        Button(
                            id: files_recheck,
                            label: tr! { "zh-Hans" => "检查目录", "en" => "Check Directories" },
                            prominence: plain,
                            height: 30
                        ) => app.recheck_locations()
                    }
                    if app.has_leftover {
                        Row(gap: 8, align: center) {
                            Text(content: tr! { "zh-Hans" => "上次任务未完成。", "en" => "A task did not finish." }, style: caption, color: status.warning)
                            Spacer
                            Button(
                                id: files_leftover,
                                label: tr! { "zh-Hans" => "去任务页", "en" => "Open Tasks" },
                                shape: chip,
                                prominence: plain,
                                height: 28
                            ) => draft.page.set(Page::Tasks)
                        }
                    }
                    if !app.has_entries {
                        Text(content: app.empty_text, style: caption, emphasis: secondary)
                    }
                }

                Table(
                    id: files,
                    label: tr! { "zh-Hans" => "文件", "en" => "Files" },
                    items: app.entries,
                    key: id
                ) |entry| {
                    TableColumn(
                        title: tr! { "zh-Hans" => "选", "en" => "Pick" },
                        width: 36
                    ) {
                        Button(
                            id: files_toggle,
                            label: entry.mark,
                            variant: rowicon,
                            height: 38,
                            width: fill
                        ) {
                            Row(gap: 0, height: 38, align: center) {
                                Spacer
                                if entry.chosen {
                                    Label(
                                        title: tr! { "zh-Hans" => "取消选择", "en" => "Deselect" },
                                        icon: "checkmark.circle.fill",
                                        display: icon,
                                        font_size: 16
                                    )
                                } else {
                                    Label(
                                        title: tr! { "zh-Hans" => "选择", "en" => "Select" },
                                        icon: "circle",
                                        display: icon,
                                        font_size: 16
                                    )
                                }
                                Spacer
                            }
                        } => app.toggle(entry.id)
                    }
                    TableColumn(
                        title: tr! { "zh-Hans" => "名称", "en" => "Name" },
                        sorted: app.by_name,
                        descending: app.descending
                    ) {
                        Column(gap: 1) {
                            Text(content: entry.name, lines: 1)
                            Text(content: entry.detail_text, style: caption)
                        }
                    } => app.sort_name()
                    TableColumn(
                        title: tr! { "zh-Hans" => "大小", "en" => "Size" },
                        width: 74,
                        sorted: app.by_size,
                        descending: app.descending
                    ) {
                        // 行主动作引用不了模板变量，进目录与开归档都放在这一格。
                        if entry.folder {
                            Button(
                                id: files_enter_dir,
                                label: tr! { "zh-Hans" => "进入文件夹", "en" => "Open Folder" },
                                variant: rowarrow,
                                height: 38,
                                width: fill
                            ) {
                                Row(gap: 0, height: 38, align: center) {
                                    Spacer
                                    Label(
                                        title: tr! { "zh-Hans" => "进入文件夹", "en" => "Open Folder" },
                                        icon: "chevron.right",
                                        display: icon,
                                        font_size: 19
                                    )
                                    Spacer
                                }
                            } => app.open_dir(entry.id)
                        } else if entry.archive {
                            Button(
                                id: files_open_archive,
                                label: tr! { "zh-Hans" => "打开归档", "en" => "Open Archive" },
                                variant: rowarrow,
                                height: 38,
                                width: fill
                            ) {
                                Row(gap: 0, height: 38, align: center) {
                                    Spacer
                                    Label(
                                        title: tr! { "zh-Hans" => "打开归档", "en" => "Open Archive" },
                                        icon: "chevron.right",
                                        display: icon,
                                        font_size: 19
                                    )
                                    Spacer
                                }
                            } => app.open_archive(entry.id, "", "")
                        } else {
                            Text(content: entry.size_text, emphasis: secondary)
                        }
                    } => app.sort_size()
                }

                Column(gap: 4, padding_x: 16, padding_y: 10) {
                    Row(gap: 8, align: center) {
                        Text(
                            content: tr! {
                                "zh-Hans" => "已选 {count} 项 · {bytes}",
                                "en" => "{count} selected · {bytes}";
                                count: app.selected, bytes: app.selected_text
                            },
                            style: caption
                        )
                        Spacer
                        Text(
                            content: tr! {
                                "zh-Hans" => "共 {count} 项 · {sort}",
                                "en" => "{count} items · {sort}";
                                count: app.count, sort: app.sort_text
                            },
                            style: caption
                        )
                    }
                    if app.status != "" {
                        Text(content: app.status, style: caption, emphasis: secondary, lines: 2)
                    }
                }
                // 内嵌的 Table 自己滚动，不像 ScrollView 那样替浮动标签栏
                // 留出底部空间；这一截把页脚和最后几行抬到导航栏之上。
                Column(
                    height: if window.compact {
                        if app.job_active || app.has_leftover { 166 } else { 98 }
                    } else if window.short && (app.job_active || app.has_leftover) {
                        100
                    } else {
                        0
                    }
                ) {}
            }
        }
    }
}

#[component]
pub fn archive_page(app: lambda::Client, #[session] draft: Draft) -> impl View {
    ui! {
        Column(gap: 4) {
            Column(gap: 8, padding_x: 16, padding_y: 6) {
                Row(gap: 8, align: center) {
                    Column(gap: 2, align: leading) {
                        Row(gap: 6, align: center) {
                            Text(content: app.archive_title, style: title2, bold: true, lines: 1)
                            if app.archive_encrypted {
                                Label(
                                    title: tr! { "zh-Hans" => "加密", "en" => "Encrypted" },
                                    icon: "shield",
                                    display: icon,
                                    font_size: 12
                                )
                            }
                        }
                        Text(content: app.archive_path, style: caption, emphasis: secondary, lines: 1)
                    }
                    Spacer
                    Button(
                        id: archive_extract,
                        label: tr! { "zh-Hans" => "解压…", "en" => "Extract…" },
                        prominence: prominent,
                        shape: capsule,
                        height: 36,
                        enabled: !app.job_active
                    ) => draft.extract_open.set(true)
                    Button(
                        id: archive_close,
                        label: tr! { "zh-Hans" => "关闭", "en" => "Close" },
                        prominence: plain,
                        shape: capsule,
                        height: 36
                    ) => app.close_archive()
                }
                Flow(gap: 4) {
                    ForEach(items: app.archive_crumbs, key: id) |crumb| {
                        Button(
                            id: archive_crumb,
                            label: crumb.name,
                            height: 30
                        ) {
                            Row(gap: 0, padding_x: 10, height: 30, align: center) {
                                Text(content: crumb.name, style: control, color: accent, lines: 1)
                            }
                        } => app.open_archive_crumb(crumb.id)
                    }
                    Button(
                        id: archive_up,
                        label: tr! { "zh-Hans" => "↑ 上级", "en" => "↑ Up" },
                        prominence: plain,
                        height: 30,
                        enabled: app.archive_can_go_up
                    ) => app.archive_up()
                    Button(
                        id: archive_select_all,
                        label: app.archive_select_all_label,
                        prominence: plain,
                        height: 30,
                        enabled: app.archive_has_entries
                    ) => app.archive_select_all()
                    Button(
                        id: archive_sort_name,
                        label: tr! { "zh-Hans" => "名称", "en" => "Name" },
                        prominence: plain,
                        height: 30
                    ) => app.archive_sort_name()
                    Button(
                        id: archive_sort_size,
                        label: tr! { "zh-Hans" => "大小", "en" => "Size" },
                        prominence: plain,
                        height: 30
                    ) => app.archive_sort_size()
                }
                if !app.archive_has_entries {
                    Text(content: app.archive_empty_text, style: caption, emphasis: secondary)
                }
            }

            Table(
                id: archive_files,
                label: tr! { "zh-Hans" => "归档内容", "en" => "Archive Contents" },
                items: app.archive_entries,
                key: id
            ) |entry| {
                TableColumn(
                    title: tr! { "zh-Hans" => "选", "en" => "Pick" },
                    width: 36
                ) {
                    Button(
                        id: archive_toggle,
                        label: entry.mark,
                        variant: rowicon,
                        height: 38,
                        width: fill
                    ) {
                        Row(gap: 0, height: 38, align: center) {
                            Spacer
                            if entry.chosen {
                                Label(
                                    title: tr! { "zh-Hans" => "取消选择", "en" => "Deselect" },
                                    icon: "checkmark.circle.fill",
                                    display: icon,
                                    font_size: 16
                                )
                            } else {
                                Label(
                                    title: tr! { "zh-Hans" => "选择", "en" => "Select" },
                                    icon: "circle",
                                    display: icon,
                                    font_size: 16
                                )
                            }
                            Spacer
                        }
                    } => app.archive_toggle(entry.id)
                }
                TableColumn(
                    title: tr! { "zh-Hans" => "名称", "en" => "Name" },
                    sorted: app.archive_by_name,
                    descending: app.archive_descending
                ) {
                    Column(gap: 1) {
                        Row(gap: 4, align: center) {
                            Text(content: entry.name, lines: 1)
                            if entry.encrypted {
                                Label(
                                    title: tr! { "zh-Hans" => "加密文件", "en" => "Encrypted file" },
                                    icon: "shield",
                                    display: icon,
                                    font_size: 11
                                )
                            }
                        }
                        Text(content: entry.detail_text, style: caption)
                    }
                } => app.archive_sort_name()
                TableColumn(
                    title: tr! { "zh-Hans" => "大小", "en" => "Size" },
                    width: 74,
                    sorted: app.archive_by_size,
                    descending: app.archive_descending
                ) {
                    if entry.folder {
                        Button(
                            id: archive_enter_dir,
                            label: tr! { "zh-Hans" => "进入文件夹", "en" => "Open Folder" },
                            variant: rowarrow,
                            height: 38,
                            width: fill
                        ) {
                            Row(gap: 0, height: 38, align: center) {
                                Spacer
                                Label(
                                    title: tr! { "zh-Hans" => "进入文件夹", "en" => "Open Folder" },
                                    icon: "chevron.right",
                                    display: icon,
                                    font_size: 19
                                )
                                Spacer
                            }
                        } => app.archive_enter(entry.id)
                    } else if entry.image {
                        Button(
                            id: archive_preview_image,
                            label: tr! { "zh-Hans" => "预览图片", "en" => "Preview Picture" },
                            variant: rowarrow,
                            height: 38,
                            width: fill,
                            enabled: !app.job_active
                        ) {
                            Row(gap: 0, height: 38, align: center) {
                                Spacer
                                Label(
                                    title: tr! { "zh-Hans" => "预览图片", "en" => "Preview Picture" },
                                    icon: "photo",
                                    display: icon,
                                    font_size: 18
                                )
                                Spacer
                            }
                        } => app.preview_open(entry.id)
                    } else if entry.preview {
                        Button(
                            id: archive_preview,
                            label: tr! { "zh-Hans" => "预览文本", "en" => "Preview Text" },
                            variant: rowarrow,
                            height: 38,
                            width: fill,
                            enabled: !app.job_active
                        ) {
                            Row(gap: 0, height: 38, align: center) {
                                Spacer
                                Label(
                                    title: tr! { "zh-Hans" => "预览文本", "en" => "Preview Text" },
                                    icon: "doc",
                                    display: icon,
                                    font_size: 18
                                )
                                Spacer
                            }
                        } => app.preview_open(entry.id)
                    } else {
                        Text(content: entry.size_text, emphasis: secondary)
                    }
                } => app.archive_sort_size()
            }

            Column(gap: 4, padding_x: 16, padding_y: 10) {
                Row(gap: 8, align: center) {
                    Text(
                        content: tr! {
                            "zh-Hans" => "已选 {count} 项 · {bytes}",
                            "en" => "{count} selected · {bytes}";
                            count: app.archive_selected, bytes: app.archive_selected_text
                        },
                        style: caption
                    )
                    Spacer
                    Text(
                        content: tr! {
                            "zh-Hans" => "共 {count} 项 · {sort}",
                            "en" => "{count} items · {sort}";
                            count: app.archive_count, sort: app.archive_sort_text
                        },
                        style: caption
                    )
                }
                if app.status != "" {
                    Text(content: app.status, style: caption, emphasis: secondary, lines: 2)
                }
            }
            // 与文件页同理：归档表也是内嵌列表，给浮动标签栏让出底部。
            Column(
                height: if window.compact {
                    if app.job_active || app.has_leftover { 166 } else { 98 }
                } else if window.short && (app.job_active || app.has_leftover) {
                    100
                } else {
                    0
                }
            ) {}
        }
    }
}

#[component]
pub fn tasks_page(app: lambda::Client, #[session] draft: Draft) -> impl View {
    ui! {
        ScrollView {
            Column(gap: 16, padding: 18) {
                Text(content: tr! { "zh-Hans" => "任务队列", "en" => "Tasks" }, style: title, bold: true)

                if app.job_active {
                    Stack(variant: card) {
                        Column(gap: 10, padding: 16, align: leading) {
                            Row(gap: 8, align: center) {
                                Text(content: app.job_title, style: headline, bold: true, lines: 1)
                                Spacer
                                Text(content: app.job_percent_text, style: caption)
                            }
                            if app.job_percent >= 0 {
                                Progress(value: app.job_percent, total: 100)
                            }
                            Row(gap: 8, align: center) {
                                Text(content: app.job_phase, style: caption, emphasis: secondary, lines: 1)
                                Spacer
                                Button(
                                    id: task_cancel,
                                    label: tr! { "zh-Hans" => "取消", "en" => "Cancel" },
                                    prominence: plain,
                                    height: 28
                                ) => app.cancel_job()
                            }
                            if app.job_label != "" {
                                Text(content: app.job_label, style: caption, emphasis: secondary, lines: 1)
                            }
                            if !app.job_cancellable {
                                Text(
                                    content: tr! {
                                        "zh-Hans" => "Android 上 7-Zip 正在执行的这一步不能中断：取消会在读取与导出的下一个检查点生效，压缩/解压调用本身要等它返回。",
                                        "en" => "On Android the running 7-Zip call cannot be interrupted: cancellation takes effect at the next check while reading or exporting, and the compress/extract call itself must return first."
                                    },
                                    style: caption,
                                    color: status.warning,
                                    lines: 4
                                )
                            }
                        }
                    }
                } else {
                    Stack(variant: card) {
                        Column(gap: 8, padding: 16, align: leading) {
                            Text(content: tr! { "zh-Hans" => "没有正在运行的任务", "en" => "No task is running" }, style: headline, bold: true)
                            Text(
                                content: tr! { "zh-Hans" => "压缩与解压从「文件」页开始。", "en" => "Start compressing or extracting from Files." },
                                style: caption,
                                emphasis: secondary
                            )
                            Button(
                                id: task_go_files,
                                label: tr! { "zh-Hans" => "去文件页", "en" => "Open Files" },
                                shape: chip,
                                prominence: plain,
                                height: 30
                            ) => draft.page.set(Page::Files)
                        }
                    }
                }

                if app.has_leftover {
                    Stack(variant: card) {
                        Column(gap: 10, padding: 16, align: leading) {
                            Text(content: tr! { "zh-Hans" => "上次任务未完成", "en" => "A task did not finish" }, style: headline, bold: true)
                            Text(content: app.leftover_text, style: caption, emphasis: secondary, lines: 3)
                            Text(
                                content: tr! {
                                    "zh-Hans" => "密码不落盘：记录里没有密码，带密码的任务重试时要再填一次。",
                                    "en" => "Passwords are not written to disk: the record has none, so a protected task needs the password again."
                                },
                                style: caption,
                                emphasis: secondary,
                                lines: 2
                            )
                            TextInput(
                                id: retry_password,
                                label: tr! { "zh-Hans" => "密码", "en" => "Password" },
                                label_hidden: true,
                                value: draft.retry_password,
                                kind: secure,
                                prompt: tr! { "zh-Hans" => "上次任务的密码（若有）", "en" => "The task's password (if any)" }
                            )
                            Flow(gap: 8) {
                                Button(
                                    id: task_retry,
                                    label: tr! { "zh-Hans" => "重试", "en" => "Retry" },
                                    prominence: prominent
                                ) => app.retry_leftover(draft.retry_password)
                                Button(
                                    id: task_dismiss,
                                    label: tr! { "zh-Hans" => "忽略", "en" => "Dismiss" },
                                    prominence: plain
                                ) => app.dismiss_leftover()
                            }
                        }
                    }
                }

                Stack(variant: card) {
                    Column(gap: 8, padding: 16, align: leading) {
                        Text(content: tr! { "zh-Hans" => "取消的边界", "en" => "What Cancelling Does" }, style: headline, bold: true)
                        Text(
                            content: tr! {
                                "zh-Hans" => "读取会逐项检查取消；7-Zip 开始压缩或解压后，只能等这一次调用返回。没有常驻前台服务，进程被系统回收时任务会断在半路，下次启动会在这一页看到未完成的记录。",
                                "en" => "Reading checks for cancellation between items; once 7-Zip starts compressing or extracting, the call must return. There is no foreground service, so if the process is reclaimed the task stops midway and its record appears here on the next launch."
                            },
                            style: caption,
                            emphasis: secondary,
                            lines: 5
                        )
                    }
                }

                if app.status != "" {
                    Text(content: app.status, style: caption, emphasis: secondary, lines: 2)
                }
            }
        }
    }
}

#[component]
pub fn passwords_page(app: lambda::Client, #[session] draft: Draft) -> impl View {
    ui! {
        ScrollView {
            Column(gap: 16, padding: 18) {
                Text(content: tr! { "zh-Hans" => "密码簿", "en" => "Passwords" }, style: title, bold: true)
                Text(
                    content: tr! {
                        "zh-Hans" => "保存的密码进入系统凭据库；Lambda 的状态与任务记录里只有标签，不保存密码本身。打开、创建、解压时可以选择一条已保存的密码。",
                        "en" => "Saved passwords go to the system credential store; Lambda's state and task records keep labels only, never the password. Saved entries can be chosen when opening, creating or extracting."
                    },
                    style: caption,
                    emphasis: secondary,
                    lines: 4
                )

                Stack(variant: card) {
                    Column(gap: 10, padding: 16, align: leading) {
                        Text(content: tr! { "zh-Hans" => "保存新密码", "en" => "Save a Password" }, style: headline, bold: true)
                        TextInput(
                            id: password_label,
                            label: tr! { "zh-Hans" => "标签", "en" => "Label" },
                            value: draft.password_label,
                            prompt: tr! { "zh-Hans" => "例如：家庭照片 2024", "en" => "For example: Family photos 2024" }
                        )
                        TextInput(
                            id: password_value,
                            label: tr! { "zh-Hans" => "密码", "en" => "Password" },
                            label_hidden: true,
                            value: draft.password_value,
                            kind: secure,
                            prompt: tr! { "zh-Hans" => "要保存的密码", "en" => "The password to save" }
                        )
                        Row(gap: 8, align: center) {
                            Button(
                                id: password_save,
                                label: tr! { "zh-Hans" => "保存到系统凭据库", "en" => "Save to Credential Store" },
                                prominence: prominent,
                                enabled: draft.password_label != "" && draft.password_value != "",
                                clears: draft.password_value
                            ) => app.save_password(draft.password_label, draft.password_value)
                        }
                        Text(
                            content: tr! {
                                "zh-Hans" => "标签会明文显示在列表里，请不要把密码本身写成标签。保存后密码框会清空。",
                                "en" => "The label is shown in plain text: do not put the password itself in it. The password field clears after saving."
                            },
                            style: caption,
                            emphasis: secondary,
                            lines: 2
                        )
                    }
                }

                if app.has_passwords {
                    Column(gap: 8, align: leading) {
                        Text(content: tr! { "zh-Hans" => "已保存", "en" => "Saved" }, style: title3, bold: true)
                        Stack(variant: card) {
                            Column(gap: 0) {
                                ForEach(items: app.passwords, key: id) |entry| {
                                    Row(gap: 10, padding_x: 14, padding_y: 10, align: center) {
                                        Column(gap: 2, align: leading) {
                                            Text(content: entry.label, lines: 1)
                                            Text(content: entry.note, style: caption, emphasis: secondary, lines: 1)
                                        }
                                        Spacer
                                        Button(
                                            id: password_forget,
                                            label: tr! { "zh-Hans" => "忘记", "en" => "Forget" },
                                            prominence: plain,
                                            role: destructive,
                                            height: 28
                                        ) => app.forget_password(entry.id)
                                    }
                                }
                            }
                        }
                        Text(
                            content: tr! {
                                "zh-Hans" => "「忘记」只删掉这条凭据，不动任何压缩包。保存的密码不能在这里查看，只能被打开、创建或解压使用。",
                                "en" => "“Forget” removes this credential only; no archive is touched. Saved passwords cannot be viewed here, only used to open, create or extract."
                            },
                            style: caption,
                            emphasis: secondary,
                            lines: 2
                        )
                    }
                } else {
                    Text(
                        content: tr! { "zh-Hans" => "还没有保存的密码。", "en" => "No saved passwords yet." },
                        style: caption,
                        emphasis: secondary
                    )
                }

                if app.status != "" {
                    Text(content: app.status, style: caption, emphasis: secondary, lines: 2)
                }
            }
        }
    }
}

#[component]
pub fn settings_page(app: lambda::Client, #[session] draft: Draft) -> impl View {
    ui! {
        ScrollView {
            Column(gap: 16, padding: 18) {
                Text(content: tr! { "zh-Hans" => "设置", "en" => "Settings" }, style: title, bold: true)

                Stack(variant: card) {
                    Column(gap: 10, padding: 12, align: leading) {
                        Row(gap: 8, align: center) {
                            Text(content: tr! { "zh-Hans" => "语言", "en" => "Language" }, style: body)
                            Spacer
                            LanguagePicker(
                                id: settings_language,
                                label: tr! { "zh-Hans" => "语言", "en" => "Language" },
                                label_hidden: true
                            )
                        }
                        Row(gap: 8, align: center) {
                            Text(content: tr! { "zh-Hans" => "外观", "en" => "Appearance" }, style: body)
                            Spacer
                            // 分段的按钮组在窄屏上很挤（Enji 上也是如此）；用
                            // chips 保持同一组选项，内边距更小，也能自然换行。
                            Picker(
                                id: settings_appearance,
                                label: tr! { "zh-Hans" => "外观", "en" => "Appearance" },
                                label_hidden: true,
                                selected: app.appearance,
                                style: chips
                            ) => app.set_appearance(app.appearance)
                        }
                        Row(gap: 8, align: center) {
                            Text(content: tr! { "zh-Hans" => "强调色", "en" => "Accent" }, style: body)
                            Spacer
                            Picker(
                                id: settings_accent,
                                label: tr! { "zh-Hans" => "强调色", "en" => "Accent" },
                                label_hidden: true,
                                selected: app.accent,
                                style: chips
                            ) => app.set_accent(app.accent)
                        }
                        Row(gap: 8, align: center) {
                            Text(content: tr! { "zh-Hans" => "版本", "en" => "Version" }, style: body)
                            Spacer
                            Text(
                                content: tr! { "zh-Hans" => "{version}", "en" => "{version}"; version: window.app_version },
                                style: caption,
                                emphasis: secondary
                            )
                        }
                        Row(gap: 8, align: center) {
                            Text(content: tr! { "zh-Hans" => "引擎", "en" => "Engine" }, style: body)
                            Spacer
                            Text(content: app.engine, style: caption, emphasis: secondary, lines: 1)
                            Button(
                                id: settings_probe,
                                label: tr! { "zh-Hans" => "重新检测", "en" => "Check Again" },
                                prominence: plain,
                                height: 28
                            ) => app.probe()
                        }
                    }
                }

                Text(content: tr! { "zh-Hans" => "目录", "en" => "Directories" }, style: title3, bold: true)
                Text(
                    content: tr! {
                        "zh-Hans" => "目录是用户授权给 Lambda 的文件夹。Android 的授权可能因重启或重装失效：失效的目录显示 ↻，重新授权同一个文件夹即可。移除目录只是丢掉授权，不会删除文件。",
                        "en" => "A directory is a folder authorized to Lambda. Android grants can expire after a reboot or reinstall: a stale directory shows ↻ and is fixed by authorizing the same folder again. Removing a directory drops the grant only; files are not deleted."
                    },
                    style: caption,
                    emphasis: secondary,
                    lines: 4
                )
                if app.has_locations {
                    Stack(variant: card) {
                        Column(gap: 0) {
                            ForEach(items: app.locations, key: id) |location| {
                                Row(gap: 10, padding_x: 14, padding_y: 10, align: center) {
                                    Column(gap: 2, align: leading) {
                                        Text(content: location.label, lines: 1)
                                        Text(content: location.note, style: caption, emphasis: secondary, lines: 1)
                                    }
                                    Spacer
                                    if location.stale {
                                        Button(
                                            id: settings_reauth,
                                            label: tr! { "zh-Hans" => "重新授权", "en" => "Authorize Again" },
                                            shape: chip,
                                            prominence: plain,
                                            pick: draft.tree,
                                            files: "folder"
                                        ) => app.add_location(draft.tree)
                                    } else {
                                        Button(
                                            id: settings_open_location,
                                            label: tr! { "zh-Hans" => "选中", "en" => "Select" },
                                            shape: chip,
                                            prominence: plain
                                        ) => app.open_location(location.id)
                                    }
                                    Button(
                                        id: settings_remove_location,
                                        label: tr! { "zh-Hans" => "移除", "en" => "Remove" },
                                        shape: chip,
                                        prominence: plain,
                                        role: destructive
                                    ) => app.remove_location(location.id)
                                }
                            }
                        }
                    }
                } else {
                    Text(content: tr! { "zh-Hans" => "还没有目录。", "en" => "No directories yet." }, style: caption, emphasis: secondary)
                }
                Flow(gap: 8) {
                    Button(
                        id: settings_add_location,
                        label: tr! { "zh-Hans" => "添加目录", "en" => "Add a Directory" },
                        shape: chip,
                        pick: draft.tree,
                        files: "folder"
                    ) => app.add_location(draft.tree)
                    Button(
                        id: settings_recheck,
                        label: tr! { "zh-Hans" => "检查所有目录", "en" => "Check All Directories" },
                        shape: chip,
                        prominence: plain
                    ) => app.recheck_locations()
                }

                Text(content: tr! { "zh-Hans" => "最近打开", "en" => "Recently Opened" }, style: title3, bold: true)
                Text(
                    content: tr! {
                        "zh-Hans" => "最近记录只保存目录与显示信息，不含密码。清空列表不会删除归档文件。",
                        "en" => "Recent records hold source paths and display details only, never passwords. Clearing the list deletes no archives."
                    },
                    style: caption,
                    emphasis: secondary,
                    lines: 2
                )
                Row(gap: 8, align: center) {
                    Button(
                        id: settings_clear_recent,
                        label: tr! { "zh-Hans" => "清空最近记录", "en" => "Clear Recent" },
                        prominence: plain,
                        role: destructive
                    ) => app.clear_recent()
                }

                Text(content: tr! { "zh-Hans" => "安全与平台限制", "en" => "Security and Platform Limits" }, style: title3, bold: true)
                Text(
                    content: tr! {
                        "zh-Hans" => "压缩包密码不进入协议状态、日志或任务记录；任务记录里的密码只会是空的。Android 的 SAF 不提供删除、重命名、移动或覆盖文件的能力，Lambda 也不会尝试这样做。",
                        "en" => "Archive passwords never enter protocol state, logs or task records; a task record's password field is always empty. Android's SAF offers no delete, rename, move or overwrite, and Lambda does not attempt them."
                    },
                    style: caption,
                    emphasis: secondary,
                    lines: 4
                )

                if app.status != "" {
                    Text(content: app.status, style: caption, emphasis: secondary, lines: 2)
                }
            }
        }
    }
}

#[component]
pub fn compress_dialog(app: lambda::Client, #[session] draft: Draft) -> impl View {
    ui! {
        Modal(width: 460) {
            Column(gap: 0) {
                Row(gap: 8, padding_x: 16, padding_y: 12, align: center) {
                    Text(content: tr! { "zh-Hans" => "新建压缩", "en" => "New Archive" }, style: title3, bold: true)
                    Spacer
                    IconButton(
                        id: compress_close,
                        icon: "xmark",
                        prominence: plain,
                        label: tr! { "zh-Hans" => "关闭", "en" => "Close" }
                    ) => draft.compress_open.set(false)
                }
                Divider
                ScrollView {
                    Column(gap: 10, padding: 16, align: leading) {
                        TextInput(
                            id: compress_name,
                            label: tr! { "zh-Hans" => "归档名", "en" => "Archive name" },
                            value: draft.compress_name,
                            prompt: tr! { "zh-Hans" => "留空则按选中项命名", "en" => "Leave empty to name it after the selection" }
                        )
                        Text(content: app.output_note, style: caption, emphasis: secondary, lines: 2)
                        Text(content: tr! { "zh-Hans" => "格式", "en" => "Format" }, style: caption, emphasis: secondary)
                        Flow(gap: 6) {
                            Button(
                                id: compress_7z,
                                label: tr! { "zh-Hans" => "7z", "en" => "7z" },
                                shape: chip,
                                prominence: plain
                            ) => app.use_7z()
                            Button(
                                id: compress_zip,
                                label: tr! { "zh-Hans" => "zip", "en" => "zip" },
                                shape: chip,
                                prominence: plain
                            ) => app.use_zip()
                            Text(
                                content: tr! { "zh-Hans" => "当前：{format}", "en" => "Now: {format}"; format: app.format_text },
                                style: caption,
                                emphasis: secondary
                            )
                        }
                        Text(content: tr! { "zh-Hans" => "级别", "en" => "Level" }, style: caption, emphasis: secondary)
                        Flow(gap: 6) {
                            Button(
                                id: compress_level_store,
                                label: tr! { "zh-Hans" => "仅存储", "en" => "Store" },
                                shape: chip,
                                prominence: plain
                            ) => app.set_level(0)
                            Button(
                                id: compress_level_fast,
                                label: tr! { "zh-Hans" => "快速", "en" => "Fast" },
                                shape: chip,
                                prominence: plain
                            ) => app.set_level(1)
                            Button(
                                id: compress_level_normal,
                                label: tr! { "zh-Hans" => "标准", "en" => "Normal" },
                                shape: chip,
                                prominence: plain
                            ) => app.set_level(5)
                            Button(
                                id: compress_level_ultra,
                                label: tr! { "zh-Hans" => "极限", "en" => "Ultra" },
                                shape: chip,
                                prominence: plain
                            ) => app.set_level(9)
                        }
                        Text(
                            content: tr! { "zh-Hans" => "当前：{level}", "en" => "Now: {level}"; level: app.level_text },
                            style: caption,
                            emphasis: secondary
                        )
                        Text(
                            content: tr! { "zh-Hans" => "加密：{note}", "en" => "Encryption: {note}"; note: app.password_note },
                            style: caption,
                            emphasis: secondary
                        )
                        if app.has_passwords {
                            Text(content: tr! { "zh-Hans" => "从密码簿选择：", "en" => "Choose from Passwords:" }, style: caption, emphasis: secondary)
                            Flow(gap: 6) {
                                ForEach(items: app.passwords, key: id) |entry| {
                                    Button(
                                        id: compress_saved,
                                        label: entry.label,
                                        shape: chip,
                                        prominence: plain
                                    ) => draft.compress_secret.set(entry.id)
                                }
                            }
                        }
                        if draft.compress_secret != "" {
                            Row(gap: 6, align: center) {
                                Text(
                                    content: tr! { "zh-Hans" => "已选一条保存的密码，将优先使用它。", "en" => "A saved password is chosen and takes priority." },
                                    style: caption,
                                    emphasis: secondary
                                )
                                Spacer
                                Button(
                                    id: compress_clear_secret,
                                    label: tr! { "zh-Hans" => "不用已存密码", "en" => "Use Typed Instead" },
                                    shape: chip,
                                    prominence: plain
                                ) => draft.compress_secret.set("")
                            }
                        }
                        TextInput(
                            id: compress_password,
                            label: tr! { "zh-Hans" => "密码", "en" => "Password" },
                            label_hidden: true,
                            value: draft.compress_password,
                            kind: secure,
                            prompt: tr! { "zh-Hans" => "密码（可留空）", "en" => "Password (optional)" }
                        )
                        if app.selected == 0 {
                            Text(
                                content: tr! { "zh-Hans" => "先在文件页勾选要压缩的项。", "en" => "Select the items to compress on the Files page first." },
                                style: caption,
                                color: status.warning,
                                lines: 2
                            )
                        }
                        if app.status != "" {
                            Text(content: app.status, style: caption, emphasis: secondary, lines: 2)
                        }
                    }
                }
                Divider
                Row(gap: 6, padding: 12, align: center) {
                    Spacer
                    Button(
                        id: compress_cancel,
                        label: tr! { "zh-Hans" => "取消", "en" => "Cancel" },
                        prominence: plain
                    ) => draft.compress_open.set(false)
                    Button(
                        id: compress_start,
                        label: tr! {
                            "zh-Hans" => "开始压缩（{count} 项）",
                            "en" => "Compress {count} Items";
                            count: app.selected
                        },
                        prominence: prominent,
                        enabled: app.can_compress
                    ) => app.compress_named(draft.compress_name, draft.compress_password, draft.compress_secret)
                }
            }
            Shortcut(key: "escape") => draft.compress_open.set(false)
        }
    }
}

#[component]
pub fn extract_dialog(app: lambda::Client, #[session] draft: Draft) -> impl View {
    ui! {
        Modal(width: 480) {
            Column(gap: 0) {
                Row(gap: 8, padding_x: 16, padding_y: 12, align: center) {
                    Text(content: tr! { "zh-Hans" => "解压", "en" => "Extract" }, style: title3, bold: true)
                    Spacer
                    IconButton(
                        id: extract_close,
                        icon: "xmark",
                        prominence: plain,
                        label: tr! { "zh-Hans" => "关闭", "en" => "Close" }
                    ) => draft.extract_open.set(false)
                }
                Divider
                ScrollView {
                    Column(gap: 10, padding: 16, align: leading) {
                        Text(content: app.archive_title, style: subheadline, emphasis: secondary, lines: 1)
                        Text(content: tr! { "zh-Hans" => "方式", "en" => "Where" }, style: caption, emphasis: secondary)
                        Flow(gap: 6) {
                            Button(
                                id: extract_policy_current,
                                label: tr! { "zh-Hans" => "当前文件夹", "en" => "Here" },
                                shape: chip,
                                prominence: plain,
                                enabled: app.policy_current_ok
                            ) => app.set_policy(0)
                            Button(
                                id: extract_policy_smart,
                                label: tr! { "zh-Hans" => "智能解压", "en" => "Smart" },
                                shape: chip,
                                prominence: plain
                            ) => app.set_policy(1)
                            Button(
                                id: extract_policy_folder,
                                label: tr! { "zh-Hans" => "归档名文件夹", "en" => "Named Folder" },
                                shape: chip,
                                prominence: plain
                            ) => app.set_policy(2)
                            Button(
                                id: extract_policy_other,
                                label: tr! { "zh-Hans" => "其他目录", "en" => "Elsewhere" },
                                shape: chip,
                                prominence: plain
                            ) => app.set_policy(3)
                        }
                        Text(
                            content: tr! { "zh-Hans" => "当前：{policy}", "en" => "Now: {policy}"; policy: app.policy_text },
                            style: caption,
                            emphasis: secondary
                        )
                        if app.extract_policy == 3 {
                            Button(
                                id: extract_target,
                                label: tr! { "zh-Hans" => "选择目标文件夹…", "en" => "Choose a Target Folder…" },
                                shape: chip,
                                pick: draft.target_tree,
                                files: "folder"
                            ) => app.set_target(draft.target_tree)
                        }
                        Text(content: app.extract_target_text, style: caption, emphasis: secondary, lines: 2)
                        if app.archive_encrypted {
                            Text(
                                content: tr! {
                                    "zh-Hans" => "归档已加密。密码留空时依次使用：密码簿 > 打开归档时的密码。",
                                    "en" => "The archive is encrypted. With the password empty, this order is used: password vault, then the password this archive was opened with."
                                },
                                style: caption,
                                emphasis: secondary,
                                lines: 3
                            )
                            if app.has_passwords {
                                Flow(gap: 6) {
                                    ForEach(items: app.passwords, key: id) |entry| {
                                        Button(
                                            id: extract_saved,
                                            label: entry.label,
                                            shape: chip,
                                            prominence: plain
                                        ) => draft.extract_secret.set(entry.id)
                                    }
                                }
                            }
                            if draft.extract_secret != "" {
                                Row(gap: 6, align: center) {
                                    Text(
                                        content: tr! { "zh-Hans" => "已选一条保存的密码，将优先使用它。", "en" => "A saved password is chosen and takes priority." },
                                        style: caption,
                                        emphasis: secondary
                                    )
                                    Spacer
                                    Button(
                                        id: extract_clear_secret,
                                        label: tr! { "zh-Hans" => "不用已存密码", "en" => "Use Typed Instead" },
                                        shape: chip,
                                        prominence: plain
                                    ) => draft.extract_secret.set("")
                                }
                            }
                            TextInput(
                                id: extract_password,
                                label: tr! { "zh-Hans" => "密码", "en" => "Password" },
                                label_hidden: true,
                                value: draft.extract_password,
                                kind: secure,
                                prompt: tr! { "zh-Hans" => "密码（可留空）", "en" => "Password (optional)" }
                            )
                        }
                        Text(
                            content: tr! {
                                "zh-Hans" => "「解压」指当前这一层；在归档根时就是整个归档。同名文件落到 SAF 时由系统自动改名，不覆盖已有文件。",
                                "en" => "“Extract” means this layer; at the archive root it is the whole archive. SAF renames name clashes instead of overwriting."
                            },
                            style: caption,
                            emphasis: secondary,
                            lines: 3
                        )
                        if app.status != "" {
                            Text(content: app.status, style: caption, emphasis: secondary, lines: 2)
                        }
                    }
                }
                Divider
                // 三个按钮用 Flow：窄窗口里自动换行，不再互相挤压；取消是
                // 纯文本按钮，左右内边距随 Material 的 TextButton 收窄。
                Column(padding_x: 12, padding_y: 12) {
                    Flow(gap: 6, align: trailing) {
                        Button(
                            id: extract_cancel,
                            label: tr! { "zh-Hans" => "取消", "en" => "Cancel" },
                            prominence: plain
                        ) => draft.extract_open.set(false)
                        Spacer
                        Button(
                            id: extract_selected,
                            label: tr! {
                                "zh-Hans" => "解压 {count} 项",
                                "en" => "Extract {count}";
                                count: app.archive_selected
                            },
                            prominence: plain,
                            enabled: app.can_extract && app.archive_selected > 0
                        ) => app.extract(false, draft.extract_password, draft.extract_secret)
                        Button(
                            id: extract_all,
                            label: tr! { "zh-Hans" => "解压", "en" => "Extract" },
                            prominence: prominent,
                            enabled: app.can_extract
                        ) => app.extract(true, draft.extract_password, draft.extract_secret)
                    }
                }
            }
            Shortcut(key: "escape") => draft.extract_open.set(false)
        }
    }
}

/// 预览的标题栏：文本与图片只换图标/标题，关闭按钮共用。
#[component]
pub fn preview_header(app: lambda::Client, picture: bool) -> impl View {
    ui! {
        Row(gap: 8, padding_x: 16, padding_y: 12, align: center) {
            if picture {
                Label(
                    title: tr! { "zh-Hans" => "图片预览", "en" => "Picture Preview" },
                    icon: "photo",
                    display: icon,
                    font_size: 16
                )
            } else {
                Label(
                    title: tr! { "zh-Hans" => "文本预览", "en" => "Text Preview" },
                    icon: "doc",
                    display: icon,
                    font_size: 16
                )
            }
            Text(content: app.preview_title, style: title3, bold: true, lines: 1)
            Spacer
            IconButton(
                id: preview_close_top,
                icon: "xmark",
                prominence: plain,
                label: tr! { "zh-Hans" => "关闭", "en" => "Close" }
            ) => app.preview_close()
        }
    }
}

#[component]
pub fn preview_dialog(app: lambda::Client) -> impl View {
    ui! {
        if app.preview_is_image {
            // 图片模态按内容定高（底部不留空）；Adaptive 在模态外选尺寸，
            // 放进无高度的 Modal 会让 Compose 量不出固有高度而铺满整屏。
            Adaptive(id: preview_picture) {
                Layout(size: compact) {
                    picture_dialog(app: app, image_width: 300, image_height: 240)
                }
                Layout(size: medium) {
                    picture_dialog(app: app, image_width: 480, image_height: 300)
                }
                Layout(size: expanded) {
                    picture_dialog(app: app, image_width: 620, image_height: 360)
                }
            }
        } else {
            text_preview_dialog(app: app)
        }
    }
}

#[component]
pub fn text_preview_dialog(app: lambda::Client) -> impl View {
    ui! {
        Modal(width: 680, height: 540) {
            Column(gap: 0) {
                preview_header(app: app, picture: false)
                Divider
                ScrollView {
                    Column(gap: 8, padding: 16, align: leading) {
                        if app.preview_text == "" {
                            if app.preview_loading {
                                Text(
                                    content: tr! { "zh-Hans" => "正在读取…", "en" => "Reading…" },
                                    style: caption,
                                    emphasis: secondary
                                )
                            } else {
                                Text(
                                    content: tr! { "zh-Hans" => "这个文件没有可显示的文字", "en" => "This file has no displayable text" },
                                    style: caption,
                                    emphasis: secondary
                                )
                            }
                        } else {
                            Text(content: app.preview_text, style: code)
                        }
                    }
                }
                Divider
                Column(gap: 6, padding: 12) {
                    // 页码单独一行居中：加载中的文字只放下面状态行，不再和
                    // 「第 N 页」挤在同一行（2026-11-07 真机）。
                    Row(gap: 8, align: center) {
                        Spacer
                        Button(
                            id: preview_prev,
                            label: tr! { "zh-Hans" => "上一页", "en" => "Previous" },
                            prominence: plain,
                            enabled: app.preview_has_prev
                        ) => app.preview_turn(-1)
                        Text(content: app.preview_page_text, style: caption, lines: 1)
                        Button(
                            id: preview_next,
                            label: tr! { "zh-Hans" => "下一页", "en" => "Next" },
                            prominence: plain,
                            enabled: app.preview_has_next
                        ) => app.preview_turn(1)
                        Spacer
                    }
                    if app.preview_status != "" {
                        Text(content: app.preview_status, style: caption, emphasis: secondary, lines: 2)
                    }
                }
            }
            Shortcut(key: "escape") => app.preview_close()
        }
    }
}

#[component]
pub fn picture_dialog(app: lambda::Client, image_width: i64, image_height: i64) -> impl View {
    ui! {
        Modal(width: 680) {
            Column(gap: 0) {
                preview_header(app: app, picture: true)
                Divider
                Column(gap: 10, padding: 16, align: center) {
                    if app.preview_loading {
                        Text(
                            content: tr! { "zh-Hans" => "正在读取图片…", "en" => "Reading the picture…" },
                            style: caption,
                            emphasis: secondary
                        )
                    }
                    Stack(variant: card) {
                        Image(
                            source: app.preview_image,
                            width: image_width,
                            height: image_height,
                            fit: contain,
                            corner: 8,
                            label: app.preview_title
                        )
                    }
                }
                Divider
                Column(gap: 6, padding: 12) {
                    // 同级只有一张图时两个方向都不可用，整行不出现。
                    if app.preview_has_prev || app.preview_has_next {
                        Row(gap: 8, align: center) {
                            Spacer
                            Button(
                                id: preview_image_prev,
                                label: tr! { "zh-Hans" => "上一张", "en" => "Previous" },
                                prominence: plain,
                                enabled: app.preview_has_prev
                            ) => app.preview_turn(-1)
                            Text(content: app.preview_page_text, style: caption, lines: 1)
                            Button(
                                id: preview_image_next,
                                label: tr! { "zh-Hans" => "下一张", "en" => "Next" },
                                prominence: plain,
                                enabled: app.preview_has_next
                            ) => app.preview_turn(1)
                            Spacer
                        }
                    }
                    if app.preview_status != "" {
                        Text(content: app.preview_status, style: caption, emphasis: secondary, lines: 2)
                    }
                }
            }
            Shortcut(key: "escape") => app.preview_close()
        }
    }
}

#[component]
pub fn unlock_dialog(app: lambda::Client, #[session] draft: Draft) -> impl View {
    ui! {
        Modal(width: 400) {
            Column(gap: 0) {
                Row(gap: 8, padding_x: 16, padding_y: 12, align: center) {
                    Text(content: tr! { "zh-Hans" => "解锁归档", "en" => "Unlock Archive" }, style: title3, bold: true)
                    Spacer
                    IconButton(
                        id: unlock_close,
                        icon: "xmark",
                        prominence: plain,
                        label: tr! { "zh-Hans" => "关闭", "en" => "Close" }
                    ) => app.dismiss_unlock()
                }
                Divider
                ScrollView {
                    Column(gap: 10, padding: 16, align: leading) {
                        Text(content: app.archive_lock_text, style: body, lines: 3)
                        if app.has_passwords {
                            Text(content: tr! { "zh-Hans" => "从密码簿选择：", "en" => "Choose from Passwords:" }, style: caption, emphasis: secondary)
                            Flow(gap: 6) {
                                ForEach(items: app.passwords, key: id) |entry| {
                                    Button(
                                        id: unlock_saved,
                                        label: entry.label,
                                        shape: chip,
                                        prominence: plain
                                    ) => draft.unlock_secret.set(entry.id)
                                }
                            }
                        }
                        if draft.unlock_secret != "" {
                            Row(gap: 6, align: center) {
                                Text(
                                    content: tr! { "zh-Hans" => "已选一条保存的密码，将优先使用它。", "en" => "A saved password is chosen and takes priority." },
                                    style: caption,
                                    emphasis: secondary
                                )
                                Spacer
                                Button(
                                    id: unlock_clear_secret,
                                    label: tr! { "zh-Hans" => "不用已存密码", "en" => "Use Typed Instead" },
                                    shape: chip,
                                    prominence: plain
                                ) => draft.unlock_secret.set("")
                            }
                        }
                        TextInput(
                            id: unlock_password,
                            label: tr! { "zh-Hans" => "归档密码", "en" => "Archive Password" },
                            label_hidden: true,
                            value: draft.unlock_password,
                            kind: secure,
                            prompt: tr! { "zh-Hans" => "输入归档密码", "en" => "Enter the archive password" }
                        )
                        if app.status != "" {
                            Text(content: app.status, style: caption, color: status.error, lines: 2)
                        }
                        Text(
                            content: tr! {
                                "zh-Hans" => "密码只用于这一次解锁，不会写进任务记录或协议状态。取消会删掉这份临时副本。",
                                "en" => "The password is used for this unlock only and is never written to task records or protocol state. Cancelling removes the temporary copy."
                            },
                            style: caption,
                            emphasis: secondary,
                            lines: 3
                        )
                    }
                }
                Divider
                Row(gap: 6, padding: 12, align: center) {
                    Spacer
                    Button(
                        id: unlock_cancel,
                        label: tr! { "zh-Hans" => "取消", "en" => "Cancel" },
                        prominence: plain
                    ) => app.dismiss_unlock()
                    Button(
                        id: unlock_go,
                        label: tr! { "zh-Hans" => "解锁并打开", "en" => "Unlock and Open" },
                        prominence: prominent
                    ) => app.unlock_archive(draft.unlock_password, draft.unlock_secret)
                }
            }
            Shortcut(key: "escape") => app.dismiss_unlock()
        }
    }
}
