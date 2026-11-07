//! Lambda 的业务层：协议命令 -> 位置表 / 浏览层 / 平台后端 -> 7-Zip 引擎。
//!
//! 这一层没有窗口代码，可以单测。M2 的职责是**浏览**：把 SAF 授权树（或桌面
//! 目录）列成一页条目、进出目录、排序、多选。压缩与归档浏览是 M3/M4 的事，
//! 引擎能力（`backend.rs`）已经在那儿了，只是还没接到命令上。
//!
//! 三条不变量：
//!
//! - `stack[0]` 永远是位置根。`export_path` 只能写到树的根（`Lambda-Product-Plan.md`
//!   2.2 公理 A），所以「当前目录」在写侧的第一层含义就是它。
//! - `items` 是当前目录的完整体，`state.entries` 是它的投影；排序只重排投影，
//!   不重新列举。
//! - `Item.url` 不出这个模块：`state` 里的 `Entry` 只带 `id`。

mod archive;
pub mod backend;
pub mod browser;
mod compress;
mod extract;
pub mod job;
pub mod locations;
pub mod passwords;
mod picture;
mod preview;
pub mod probe;
pub mod recent;
pub mod record;
mod settings;

use std::collections::BTreeSet;
use std::path::PathBuf;

use bern::files;
use bern::media::MediaSource;
use bern::CommandError;
use lambda_protocol::lambda::{self, Commands, State};

use archive::Sort as ArchiveSort;
use browser::{Item, Sort};
use job::{Job, Outcome};
use locations::Record;

const READY: &str = "准备就绪：添加一个目录开始浏览";
const SELECT_ALL: &str = "全选";
const CLEAR_ALL: &str = "全不选";
/// 压缩级别的默认档：`-mx` 接受 0–9，界面只给「快速 1 / 标准 5 / 极限 9」
/// 三档——手机上一次极限压缩是几分钟的事，档位越多越像在让人做无用选择。
const LEVEL_BALANCED: i64 = 5;

/// 导航栈的一节。`url` 是 SAF 的 document URI 或桌面路径。
struct Step {
    name: String,
    url: String,
}

/// 一个打开着的归档。`password` 只活在这个结构里，进程退出就没了：它不
/// 进 state、不进任务记录、不进日志。
struct OpenArchive {
    title: String,
    /// 7-Zip 读的本地副本；桌面上可能就是原文件。
    local: PathBuf,
    /// 只有复制过时才有的工作目录，关闭时整个删掉。
    owned: Option<PathBuf>,
    entries: Vec<backend::ListedEntry>,
    /// 当前层，从根往下的一段名字。
    prefix: Vec<String>,
    selected: BTreeSet<i64>,
    sort: ArchiveSort,
    descending: bool,
    password: Option<String>,
    encrypted: bool,
}

/// 加密归档在等密码时，临时副本还不能删——解开后还要列目录。
struct PendingArchive {
    title: String,
    source_url: String,
    local: PathBuf,
    owned: Option<PathBuf>,
    size: u64,
}

pub struct LambdaEngine {
    state: State,
    table: Vec<Record>,
    items: Vec<Item>,
    stack: Vec<Step>,
    /// 授权失效过的位置 id；`recheck_locations` 会重算。
    stale: BTreeSet<i64>,
    /// 表格里选中的 id。界面通过 `selection_changed` 同步过来，引擎据此算
    /// 计数与合计字节（`draft.chosen` 只在界面那一侧）。
    selected_ids: BTreeSet<i64>,
    sort: Sort,
    descending: bool,
    /// 正在跑的任务。`None` 表示空闲；任务的结局由 `wake` 取走。
    job: Option<job::Shared>,
    /// 正在跑的是解压：失败文案与取消说明跟着变。
    job_is_extract: bool,
    /// 上次没跑完的任务（`record.rs`）。有值时界面多一行「重试 / 忽略」。
    leftover: Option<record::Saved>,
    archive: Option<OpenArchive>,
    pending: Option<PendingArchive>,
    /// 「其他位置」的目标树根。
    target: Option<(String, String)>,
    recent: Vec<recent::Record>,
    passwords: Vec<passwords::Record>,
    /// 打开着的文本或图片预览。密码只活在工作线程/流会话的闭包里。
    preview: Option<Preview>,
}

enum Preview {
    Text(preview::Session),
    Image(ImagePreview),
}

/// 图片预览的会话，连同当前层里可以预览的同级图片：上一张 / 下一张在
/// 同一份列表里换流，不重新列归档。
struct ImagePreview {
    session: picture::Session,
    items: Vec<archive::Item>,
    index: usize,
    /// 换图片时新流要用同一个 waker。
    waker: bern::wake::Waker,
}

impl Preview {
    fn stop(&self) {
        match self {
            Preview::Text(session) => session.stop(),
            Preview::Image(image) => image.session.stop(),
        }
    }
}

/// 一个空状态。默认构造与单测共用一份，新加字段只改这里。
fn base_state() -> State {
    let (appearance, accent) = settings::load();
    State {
        locations: Vec::new(),
        entries: Vec::new(),
        crumbs: Vec::new(),
        path: String::new(),
        status: READY.into(),
        engine: backend::probe_engine().unwrap_or_else(|error| error),
        appearance,
        accent,
        count: 0,
        selected: 0,
        selected_bytes: 0,
        selected_text: "0 B".into(),
        empty_text: String::new(),
        sort_text: String::new(),
        select_all_label: SELECT_ALL.into(),
        by_name: true,
        by_size: false,
        by_time: false,
        descending: false,
        has_locations: false,
        has_entries: false,
        can_go_up: false,
        needs_location: true,
        busy: false,
        job_active: false,
        job_title: String::new(),
        job_phase: String::new(),
        job_label: String::new(),
        job_done: 0,
        job_total: 0,
        job_percent: 0,
        job_percent_text: String::new(),
        job_cancellable: true,
        format_text: "7z".into(),
        format_7z: true,
        format_zip: false,
        level_text: format!("级别 · {}", level_text(LEVEL_BALANCED)),
        level: LEVEL_BALANCED,
        compress_label: String::new(),
        output_note: String::new(),
        password_note: password_note(true),
        can_compress: false,
        has_leftover: false,
        leftover_text: String::new(),
        archive_open: false,
        archive_locked: false,
        archive_lock_text: String::new(),
        archive_title: String::new(),
        archive_path: String::new(),
        archive_crumbs: Vec::new(),
        archive_entries: Vec::new(),
        archive_count: 0,
        archive_selected: 0,
        archive_selected_bytes: 0,
        archive_selected_text: "0 B".into(),
        archive_empty_text: String::new(),
        archive_sort_text: "名称 ↑".into(),
        archive_select_all_label: SELECT_ALL.into(),
        archive_has_entries: false,
        archive_can_go_up: false,
        archive_by_name: true,
        archive_by_size: false,
        archive_descending: false,
        archive_encrypted: false,
        extract_label: "先选要解压的项".into(),
        can_extract: false,
        extract_policy: 1,
        policy_text: "智能解压".into(),
        policy_current_ok: true,
        extract_target_text: String::new(),
        extract_has_target: false,
        recent: Vec::new(),
        has_recent: false,
        passwords: Vec::new(),
        has_passwords: false,
        preview_open: false,
        preview_title: String::new(),
        preview_text: String::new(),
        preview_page_text: String::new(),
        preview_page: 0,
        preview_loading: false,
        preview_has_prev: false,
        preview_has_next: false,
        preview_status: String::new(),
        preview_encoding: String::new(),
        preview_is_image: false,
        preview_image: MediaSource::new(""),
    }
}

impl Default for LambdaEngine {
    fn default() -> Self {
        let mut engine = Self {
            state: base_state(),
            table: locations::load(),
            items: Vec::new(),
            stack: Vec::new(),
            stale: BTreeSet::new(),
            selected_ids: BTreeSet::new(),
            sort: Sort::Name,
            descending: false,
            job: None,
            job_is_extract: false,
            leftover: None,
            archive: None,
            pending: None,
            target: None,
            recent: recent::load(),
            passwords: passwords::load(),
            preview: None,
        };
        // 5.5 ④：上次没能收尾的工作目录先清掉，别让 cache 一直占着。
        record::sweep_staging();
        // 5.5 ①②：读到一份没写完的记录，就是上次进程被杀在半路。给一次
        // 重试，而不是让用户对着「什么都没发生」重新选一遍。
        if let Some(leftover) = record::load() {
            engine.state.leftover_text = leftover.note();
            engine.state.has_leftover = true;
            engine.leftover = Some(leftover);
        }
        engine.sync_locations();
        engine.sync_recent();
        engine.sync_passwords();
        // 上次停在第一个位置：启动就能看到内容，符合「文件管理器」的预期。
        if let Some(first) = engine.table.first().cloned() {
            let _ = engine.enter_location(&first);
        }
        engine.sync_extract();
        engine
    }
}

impl lambda::Commands for LambdaEngine {
    fn add_location(&mut self, tree: MediaSource) -> Result<(), CommandError> {
        if tree.url.is_empty() {
            return Err(CommandError::new("location", "没有拿到文件夹"));
        }
        let label = files::name(&tree.url).unwrap_or_else(|| "目录".into());
        let table = locations::upsert(&tree.url, &label)
            .map_err(|error| CommandError::new("location", error))?;
        self.table = table;
        self.stale.remove(&locations::id_of(&tree.url));
        self.sync_locations();
        let record = Record::new(&tree.url, &label);
        let record = self
            .table
            .iter()
            .find(|item| item.uri == record.uri)
            .cloned()
            .unwrap_or(record);
        self.enter_location(&record)?;
        self.state.status = format!("已添加目录「{}」，共 {} 项", record.label, self.items.len());
        Ok(())
    }

    fn open_location(&mut self, id: i64) -> Result<(), CommandError> {
        let Some(record) = self.table.iter().find(|item| item.id == id).cloned() else {
            return Err(CommandError::new("location", "这个目录已经不在表里了"));
        };
        self.enter_location(&record)
    }

    fn remove_location(&mut self, id: i64) -> Result<(), CommandError> {
        self.table =
            locations::remove(id).map_err(|error| CommandError::new("location", error))?;
        self.stale.remove(&id);
        if self.current_location() == Some(id) {
            self.stack.clear();
            self.items.clear();
            self.state.status = "目录已移除".into();
            self.rebuild();
        }
        self.sync_locations();
        Ok(())
    }

    fn recheck_locations(&mut self) -> Result<(), CommandError> {
        self.state.busy = true;
        let mut usable = 0;
        let mut broken = Vec::new();
        for record in self.table.clone() {
            match files::list(&record.uri) {
                Ok(_) => {
                    usable += 1;
                    self.stale.remove(&record.id);
                }
                Err(_) => {
                    self.stale.insert(record.id);
                    broken.push(record.label);
                }
            }
        }
        self.state.busy = false;
        self.sync_locations();
        self.state.status = if broken.is_empty() {
            format!("{usable} 个目录都可用")
        } else {
            format!(
                "{usable} 个目录可用，{} 个失效（{}）",
                broken.len(),
                broken.join("、")
            )
        };
        // 当前目录若已失效，条目也要跟着清掉，否则界面会显示一页点不动的行。
        if self.current_location().is_some_and(|id| self.stale.contains(&id)) {
            self.items.clear();
            self.rebuild();
        }
        Ok(())
    }

    fn open_dir(&mut self, id: i64) -> Result<(), CommandError> {
        let Some(item) = self.items.iter().find(|item| item.id == id).cloned() else {
            return Err(CommandError::new("browse", "这一项已经不在当前目录里了"));
        };
        if !item.folder {
            return self.open(id);
        }
        self.stack.push(Step {
            name: item.name,
            url: item.url,
        });
        self.list_current()
    }

    fn open_crumb(&mut self, id: i64) -> Result<(), CommandError> {
        let index = id.max(0) as usize;
        if index + 1 >= self.stack.len() {
            return Ok(());
        }
        self.stack.truncate(index + 1);
        self.list_current()
    }

    fn go_up(&mut self) -> Result<(), CommandError> {
        if self.stack.len() <= 1 {
            self.state.status = "已经在目录根了".into();
            return Ok(());
        }
        self.stack.pop();
        self.list_current()
    }

    fn refresh(&mut self) -> Result<(), CommandError> {
        self.list_current()
    }

    fn sort_name(&mut self) -> Result<(), CommandError> {
        self.set_sort(Sort::Name);
        Ok(())
    }

    fn sort_size(&mut self) -> Result<(), CommandError> {
        self.set_sort(Sort::Size);
        Ok(())
    }

    fn sort_time(&mut self) -> Result<(), CommandError> {
        self.set_sort(Sort::Time);
        Ok(())
    }

    fn toggle(&mut self, id: i64) -> Result<(), CommandError> {
        if !self.selected_ids.remove(&id) {
            self.selected_ids.insert(id);
        }
        // 只改这一行的标记：一万项的目录里重建整页是几十毫秒的浪费。
        let chosen = self.selected_ids.contains(&id);
        if let Some(entry) = self.state.entries.iter_mut().find(|entry| entry.id == id) {
            entry.chosen = chosen;
            entry.mark = mark(chosen).into();
        }
        self.apply_selection();
        self.state.select_all_label = self.select_all_text();
        Ok(())
    }

    fn select_all(&mut self) -> Result<(), CommandError> {
        let all = !self.items.is_empty() && self.selected_ids.len() == self.items.len();
        self.selected_ids = if all {
            BTreeSet::new()
        } else {
            self.items.iter().map(|item| item.id).collect()
        };
        let chosen = !all;
        for entry in &mut self.state.entries {
            entry.chosen = chosen;
            entry.mark = mark(chosen).into();
        }
        self.apply_selection();
        self.state.select_all_label = self.select_all_text();
        // 状态行不写选择数：底部已经有「已选 N 项」，再写一遍会把
        // 「「many」共 10001 项」这条目录描述覆盖掉（2026-11-06 真机）。
        Ok(())
    }

    fn open(&mut self, id: i64) -> Result<(), CommandError> {
        let Some(item) = self.items.iter().find(|item| item.id == id).cloned() else {
            return Ok(());
        };
        if item.folder {
            return self.open_dir(item.id);
        }
        if item.archive {
            return self.open_archive(item.id, String::new(), String::new());
        }
        self.state.status = format!("Lambda 不打开普通文件：{}", item.name);
        Ok(())
    }

    fn probe(&mut self) -> Result<(), CommandError> {
        match backend::probe_engine() {
            Ok(name) => {
                self.state.engine = name;
                self.state.status = "引擎已就位".into();
                Ok(())
            }
            Err(message) => {
                self.state.engine = message.clone();
                self.state.status = message.clone();
                Err(CommandError::new("engine", message))
            }
        }
    }

    fn clear(&mut self) -> Result<(), CommandError> {
        self.state.status = READY.into();
        self.state.selected = 0;
        self.state.selected_bytes = 0;
        self.state.busy = false;
        Ok(())
    }

    fn boundary(&mut self) -> Result<(), CommandError> {
        self.state.busy = true;
        self.state.status = "边界补测运行中（异常归档 + 32 MB 计时）…".into();
        // 报告的公有出口就是当前位置的树根：`export_path` 只能写树根。
        let root = self.stack.first().map(|step| step.url.clone());
        let report = probe::run(root.as_deref());
        self.state.busy = false;
        self.state.status = report
            .lines()
            .find(|line| line.starts_with("判定"))
            .unwrap_or("边界补测完成（没有判定行）")
            .trim()
            .to_string();
        Ok(())
    }

    fn use_7z(&mut self) -> Result<(), CommandError> {
        self.set_format(true);
        Ok(())
    }

    fn use_zip(&mut self) -> Result<(), CommandError> {
        self.set_format(false);
        Ok(())
    }

    fn set_level(&mut self, level: i64) -> Result<(), CommandError> {
        self.state.level = level.clamp(0, 9);
        self.state.level_text = format!("级别 · {}", level_text(self.state.level));
        Ok(())
    }

    fn toggle_format(&mut self) -> Result<(), CommandError> {
        self.set_format(!self.state.format_7z);
        Ok(())
    }

    fn cycle_level(&mut self) -> Result<(), CommandError> {
        // 1 → 5 → 9 → 1：三档一轮，点两下就能从快速到极限。
        let next = match self.state.level {
            0..=3 => 5,
            4..=6 => 9,
            _ => 1,
        };
        self.set_level(next)
    }

    fn compress(&mut self, password: String) -> Result<(), CommandError> {
        self.compress_selected(None, password, String::new())
    }

    /// 重跑上次没跑完的任务。密码不在记录里，得用户重新给。
    fn retry_leftover(&mut self, password: String) -> Result<(), CommandError> {
        if self.job.is_some() {
            return Err(CommandError::new(
                "job_busy",
                "已有任务在运行：等它结束，或先取消",
            ));
        }
        let Some(saved) = self.leftover.clone() else {
            return Err(CommandError::new("job", "没有可重试的任务"));
        };
        if saved.locked && password.is_empty() {
            return Err(CommandError::new(
                "job",
                "上次的任务带密码，密码不落盘：先在密码框里填上再重试",
            ));
        }
        let inputs: Vec<compress::Input> = saved
            .inputs
            .iter()
            .map(|input| compress::Input {
                name: input.name.clone(),
                url: input.url.clone(),
                folder: input.folder,
            })
            .collect();
        let plan = compress::Plan {
            kind: if saved.kind.eq_ignore_ascii_case("zip") {
                "zip"
            } else {
                "7z"
            },
            level: saved.level.min(9),
            password,
            root: saved.root.clone(),
            name: saved.name.clone(),
        };
        // 格式与级别跟着记录走，界面上的按钮也要跟着变，不然下一句话就和
        // 实际发生的事对不上。
        self.set_format(plan.kind == "7z");
        let _ = self.set_level(plan.level as i64);
        self.spawn(inputs, plan)
    }

    fn dismiss_leftover(&mut self) -> Result<(), CommandError> {
        record::discard();
        self.leftover = None;
        self.state.has_leftover = false;
        self.state.leftover_text.clear();
        self.state.status = "已忽略上次未完成的任务记录".into();
        Ok(())
    }

    fn open_archive(
        &mut self,
        id: i64,
        password: String,
        secret: String,
    ) -> Result<(), CommandError> {
        let Some(item) = self.items.iter().find(|item| item.id == id).cloned() else {
            return Err(CommandError::new("archive", "这一项已经不在当前目录里了"));
        };
        if item.folder {
            return Err(CommandError::new("archive", "文件夹请直接进入"));
        }
        if !item.archive {
            return Err(CommandError::new("archive", "这一项不是 Lambda 认识的归档"));
        }
        self.stage_open(item.name, item.url, item.size, password, secret)
    }

    fn open_recent(
        &mut self,
        id: i64,
        password: String,
        secret: String,
    ) -> Result<(), CommandError> {
        let Some(record) = self.recent.iter().find(|record| record.id == id).cloned() else {
            return Err(CommandError::new("archive", "这条最近记录已经不在了"));
        };
        self.stage_open(record.name, record.url, record.size, password, secret)
    }

    fn unlock_archive(
        &mut self,
        password: String,
        secret: String,
    ) -> Result<(), CommandError> {
        let Some(pending) = self.pending.as_ref() else {
            return Err(CommandError::new("archive", "没有等待解锁的归档"));
        };
        let password = self.solve_password(password, secret)?;
        if password.is_empty() {
            return Err(CommandError::new("archive_password", "先输入归档密码"));
        }
        let local = pending.local.clone();
        let title = pending.title.clone();
        let source_url = pending.source_url.clone();
        let size = pending.size;
        let owned = pending.owned.clone();
        match backend::list_entries(&local, Some(&password)) {
            Ok(entries) => {
                self.finish_open(title, source_url, local, owned, size, entries, password);
                Ok(())
            }
            Err(message) if archive::password_error(&message) => {
                self.state.status = format!("密码不对：「{title}」没有解开");
                Err(CommandError::new("archive_password", message))
            }
            Err(message) => {
                self.cleanup_archive();
                self.pending = None;
                self.state.archive_locked = false;
                self.state.archive_lock_text.clear();
                self.sync_archive();
                Err(CommandError::new("archive", message))
            }
        }
    }

    fn dismiss_unlock(&mut self) -> Result<(), CommandError> {
        self.cleanup_archive();
        self.pending = None;
        self.state.archive_locked = false;
        self.state.archive_lock_text.clear();
        self.sync_archive();
        self.state.status = "已取消打开加密归档".into();
        Ok(())
    }

    fn archive_enter(&mut self, id: i64) -> Result<(), CommandError> {
        let Some(item) = self.layer_items().into_iter().find(|item| item.id == id) else {
            return Err(CommandError::new("archive", "这一项已经不在归档的当前层里了"));
        };
        if !item.folder {
            self.state.status = format!("「{}」是文件：勾选它可以解压", item.name);
            return Ok(());
        }
        self.stop_preview();
        let Some(archive) = self.archive.as_mut() else {
            return Err(CommandError::new("archive", "没有打开归档"));
        };
        archive.prefix.push(item.name);
        archive.selected.clear();
        self.sync_archive();
        self.state.status = self.state.archive_path.clone();
        Ok(())
    }

    fn archive_up(&mut self) -> Result<(), CommandError> {
        self.stop_preview();
        let Some(archive) = self.archive.as_mut() else {
            return Err(CommandError::new("archive", "没有打开归档"));
        };
        if archive.prefix.pop().is_some() {
            archive.selected.clear();
            self.sync_archive();
        } else {
            self.state.status = "已经在归档根了".into();
        }
        Ok(())
    }

    fn open_archive_crumb(&mut self, id: i64) -> Result<(), CommandError> {
        self.stop_preview();
        let Some(archive) = self.archive.as_mut() else {
            return Err(CommandError::new("archive", "没有打开归档"));
        };
        let depth = id.max(0) as usize;
        if depth >= archive.prefix.len() {
            return Ok(());
        }
        archive.prefix.truncate(depth);
        archive.selected.clear();
        self.sync_archive();
        Ok(())
    }

    fn close_archive(&mut self) -> Result<(), CommandError> {
        self.cleanup_archive();
        self.archive = None;
        self.pending = None;
        self.state.archive_locked = false;
        self.state.archive_lock_text.clear();
        self.sync_archive();
        self.state.status = "已关闭归档，回到文件列表".into();
        Ok(())
    }

    fn archive_toggle(&mut self, id: i64) -> Result<(), CommandError> {
        let Some(archive) = self.archive.as_mut() else {
            return Err(CommandError::new("archive", "没有打开归档"));
        };
        if !archive.selected.remove(&id) {
            archive.selected.insert(id);
        }
        let chosen = archive.selected.contains(&id);
        if let Some(entry) = self
            .state
            .archive_entries
            .iter_mut()
            .find(|entry| entry.id == id)
        {
            entry.chosen = chosen;
            entry.mark = mark(chosen).into();
        }
        self.apply_archive_selection();
        Ok(())
    }

    fn archive_select_all(&mut self) -> Result<(), CommandError> {
        let items = self.layer_items();
        let Some(archive) = self.archive.as_mut() else {
            return Err(CommandError::new("archive", "没有打开归档"));
        };
        let all = !items.is_empty() && archive.selected.len() == items.len();
        archive.selected = if all {
            BTreeSet::new()
        } else {
            items.iter().map(|item| item.id).collect()
        };
        let chosen = !all;
        for entry in &mut self.state.archive_entries {
            entry.chosen = chosen;
            entry.mark = mark(chosen).into();
        }
        self.apply_archive_selection();
        Ok(())
    }

    fn archive_sort_name(&mut self) -> Result<(), CommandError> {
        self.set_archive_sort(ArchiveSort::Name);
        Ok(())
    }

    fn archive_sort_size(&mut self) -> Result<(), CommandError> {
        self.set_archive_sort(ArchiveSort::Size);
        Ok(())
    }

    fn set_policy(&mut self, policy: i64) -> Result<(), CommandError> {
        let policy = policy.clamp(0, 3);
        if policy == 0 && self.stack.len() != 1 {
            return Err(CommandError::new(
                "extract",
                "「当前文件夹」只在目录根可用：SAF 只能写到树的根目录",
            ));
        }
        self.state.extract_policy = policy;
        self.sync_extract();
        self.state.status = format!("解压方式：{}", self.state.policy_text);
        Ok(())
    }

    fn set_target(&mut self, tree: MediaSource) -> Result<(), CommandError> {
        if tree.url.is_empty() {
            return Err(CommandError::new("extract", "没有拿到目标文件夹"));
        }
        let label = files::name(&tree.url).unwrap_or_else(|| "目标目录".into());
        self.target = Some((tree.url, label.clone()));
        self.state.extract_policy = 3;
        self.sync_extract();
        self.state.status = format!("解压目标已设为「{label}」");
        Ok(())
    }

    fn extract(
        &mut self,
        all: bool,
        password: String,
        secret: String,
    ) -> Result<(), CommandError> {
        if self.job.is_some() {
            return Err(CommandError::new(
                "job_busy",
                "已有任务在运行：等它结束，或先取消",
            ));
        }
        let Some(archive) = self.archive.as_ref() else {
            return Err(CommandError::new("extract", "先打开一个归档"));
        };
        let policy = self.state.extract_policy;
        if policy == 0 && self.stack.len() != 1 {
            return Err(CommandError::new(
                "extract",
                "「当前文件夹」只在目录根可用：换一种解压方式，或先回到目录根",
            ));
        }
        // 先抄出来再碰 `archive` 的可变借用：后面还要用这些值拼路径与计划。
        let prefix = archive.prefix.join("/");
        let title = archive.title.clone();
        let stem = archive::stem_of(&title);
        let encrypted = archive.encrypted;
        let stored_password = archive.password.clone();
        let selected = archive.selected.clone();
        let local = archive.local.clone();
        let chosen: Vec<archive::Item> = if all {
            self.layer_items()
        } else {
            self.layer_items()
                .into_iter()
                .filter(|item| selected.contains(&item.id))
                .collect()
        };
        if chosen.is_empty() {
            return Err(CommandError::new("extract", "先勾选要解压的项"));
        }
        let (root, target_label) = if policy == 3 {
            self.target
                .clone()
                .ok_or_else(|| CommandError::new("extract", "先选一个目标目录"))?
        } else {
            let step = self
                .stack
                .first()
                .ok_or_else(|| CommandError::new("extract", "先选一个目录：解压结果要落到它里面"))?;
            (step.url.clone(), step.name.clone())
        };
        let typed = self.solve_password(password, secret)?;
        let password = if typed.is_empty() {
            stored_password.unwrap_or_default()
        } else {
            typed
        };
        if encrypted && password.is_empty() {
            return Err(CommandError::new(
                "extract",
                "这个归档已加密：先输入密码，或从密码簿里选一条",
            ));
        }
        let paths = if all && prefix.is_empty() {
            Vec::new()
        } else if all {
            vec![prefix]
        } else {
            chosen.iter().map(|item| item.path.clone()).collect()
        };
        let outputs = chosen
            .iter()
            .map(|item| extract::Output {
                path: item.path.clone(),
                folder: item.folder,
            })
            .collect::<Vec<_>>();
        if let Some(archive) = self.archive.as_mut()
            && archive.password.is_none()
            && !password.is_empty()
        {
            // 只记住这一次打开的密码；关闭归档或退出进程就没了。
            archive.password = Some(password.clone());
        }
        let plan = extract::Plan {
            archive: local,
            password,
            paths,
            outputs,
            root,
            target_label: target_label.clone(),
            policy,
            stem,
        };
        self.spawn_extract(plan, chosen.len() as i64, &target_label)
    }

    fn compress_named(
        &mut self,
        name: String,
        password: String,
        secret: String,
    ) -> Result<(), CommandError> {
        self.compress_selected(Some(name), password, secret)
    }

    fn save_password(
        &mut self,
        label: String,
        password: String,
    ) -> Result<(), CommandError> {
        let record = passwords::save(&label, &password)?;
        self.passwords = passwords::load();
        self.sync_passwords();
        self.state.status = format!("已保存密码「{}」到系统凭据库", record.label);
        Ok(())
    }

    fn forget_password(&mut self, id: String) -> Result<(), CommandError> {
        passwords::forget(&id)?;
        self.passwords = passwords::load();
        self.sync_passwords();
        self.state.status = "已忘掉这条保存的密码".into();
        Ok(())
    }

    fn clear_recent(&mut self) -> Result<(), CommandError> {
        recent::clear();
        self.recent.clear();
        self.sync_recent();
        self.state.status = "最近打开记录已清空".into();
        Ok(())
    }

    fn open_file(
        &mut self,
        file: MediaSource,
        password: String,
        secret: String,
    ) -> Result<(), CommandError> {
        if file.url.is_empty() {
            return Err(CommandError::new("archive", "请先选择一个归档文件"));
        }
        let title = file.file_name().unwrap_or_else(|| "archive".into());
        let size = match file.open() {
            Ok(source) => source.len(),
            Err(error) => return Err(error),
        };
        self.stage_open(title, file.url, size, password, secret)
    }

    fn cancel_job(&mut self) -> Result<(), CommandError> {
        let Some(shared) = &self.job else {
            return Err(CommandError::new("job", "没有正在运行的任务"));
        };
        Job::request_cancel(shared);
        self.state.job_phase = "正在取消".into();
        self.state.status = "已请求取消：读取与导出立即停，压缩要等 7-Zip 返回".into();
        Ok(())
    }

    fn set_appearance(&mut self, appearance: lambda::Appearance) -> Result<(), CommandError> {
        self.state.appearance = appearance;
        let label = match &self.state.appearance {
            lambda::Appearance::Light => "外观：浅色",
            lambda::Appearance::Dark => "外观：深色",
            lambda::Appearance::System => "外观：跟随系统",
        };
        self.state.status = label.into();
        settings::save(&self.state.appearance, &self.state.accent);
        Ok(())
    }

    fn set_accent(&mut self, accent: lambda::Accent) -> Result<(), CommandError> {
        self.state.accent = accent;
        let label = match &self.state.accent {
            lambda::Accent::Brown => "强调色：棕色",
            lambda::Accent::Blue => "强调色：蓝色",
            lambda::Accent::Red => "强调色：红色",
        };
        self.state.status = label.into();
        settings::save(&self.state.appearance, &self.state.accent);
        Ok(())
    }

    fn preview_open(&mut self, id: i64) -> Result<(), CommandError> {
        let Some(archive) = self.archive.as_ref() else {
            return Err(CommandError::new("preview", "先打开一个归档"));
        };
        let mut items = archive::layer(&archive.entries, &self.archive_prefix());
        archive::sort(&mut items, archive.sort, archive.descending);
        let Some(item) = items.iter().find(|item| item.id == id).cloned() else {
            return Err(CommandError::new(
                "preview",
                "这一项已经不在归档的当前层里了",
            ));
        };
        if item.folder {
            return Err(CommandError::new("preview", "文件夹不能预览"));
        }
        if !archive::is_text(&item.name) && !archive::is_image(&item.name) {
            return Err(CommandError::new(
                "preview",
                "这个文件不支持预览：文本（txt / xml / json / log / csv / md 等）或图片（png / jpg / gif / webp / bmp）",
            ));
        }
        if self.job.is_some() {
            return Err(CommandError::new(
                "preview",
                "有任务在运行：等它结束再预览，7-Zip 调用是串行的",
            ));
        }
        let local = archive.local.clone();
        let password = archive.password.clone();
        if item.encrypted && password.as_deref().unwrap_or("").is_empty() {
            return Err(CommandError::new(
                "preview",
                "这个文件已加密：先在「解压…」里输入密码，再回来预览",
            ));
        }
        let Some(waker) = bern::wake::waker() else {
            return Err(CommandError::new(
                "preview",
                "引擎没有 waker，预览无法后台读取",
            ));
        };
        self.stop_preview();
        let title = item.name;
        if archive::is_image(&title) {
            // 上一张 / 下一张按归档表当前顺序走；没密码解不开的加密图片
            // 不进来，免得翻到它只剩一条报错。
            let siblings: Vec<archive::Item> = items
                .into_iter()
                .filter(|entry| {
                    !entry.folder
                        && archive::is_image(&entry.name)
                        && !(entry.encrypted && password.as_deref().unwrap_or("").is_empty())
                })
                .collect();
            let index = siblings
                .iter()
                .position(|entry| entry.id == item.id)
                .unwrap_or(0);
            let session = picture::start(
                local,
                password,
                item.path,
                title.clone(),
                item.size,
                waker.clone(),
            );
            self.preview = Some(Preview::Image(ImagePreview {
                session,
                items: siblings,
                index,
                waker,
            }));
            self.sync_preview();
        } else {
            let session = preview::start(local, password, item.path, title.clone(), item.size, waker);
            self.preview = Some(Preview::Text(session));
            self.state.preview_open = true;
            self.state.preview_is_image = false;
            self.state.preview_image = MediaSource::new("");
            self.state.preview_title = title;
            self.state.preview_page_text = "读取中…".into();
            self.state.preview_loading = true;
            self.sync_preview();
        }
        Ok(())
    }

    fn preview_close(&mut self) -> Result<(), CommandError> {
        self.stop_preview();
        Ok(())
    }

    fn preview_turn(&mut self, delta: i64) -> Result<(), CommandError> {
        if let Some(Preview::Text(session)) = self.preview.as_ref() {
            if session.turn(delta) {
                self.sync_preview();
            }
            return Ok(());
        }
        let Some(Preview::Image(image)) = self.preview.as_mut() else {
            return Ok(());
        };
        let target = image.index as i64 + delta;
        if target < 0 || target as usize >= image.items.len() {
            return Ok(());
        }
        let target = target as usize;
        let Some(archive) = self.archive.as_ref() else {
            return Ok(());
        };
        let item = image.items[target].clone();
        let session = picture::start(
            archive.local.clone(),
            archive.password.clone(),
            item.path,
            item.name,
            item.size,
            image.waker.clone(),
        );
        image.session.stop();
        image.session = session;
        image.index = target;
        self.sync_preview();
        Ok(())
    }
}

impl LambdaEngine {
    /// 打开一个归档：先复制到私有 staging（SAF 文档没有本地路径），再列。
    /// 加密归档列不动时进入「等密码」，临时副本留到解锁或取消。
    fn stage_open(
        &mut self,
        title: String,
        url: String,
        size: u64,
        password: String,
        secret: String,
    ) -> Result<(), CommandError> {
        let password = self.solve_password(password, secret)?;
        self.cleanup_archive();
        self.archive = None;
        self.pending = None;
        self.state.archive_locked = false;
        self.state.archive_lock_text.clear();
        self.sync_archive();

        let dir = backend::scratch().join(format!("open-{}", stamp()));
        if let Err(error) = std::fs::create_dir_all(&dir) {
            return Err(CommandError::new(
                "archive",
                format!("无法创建工作目录：{error}"),
            ));
        }
        let local = match backend::copy_file_to(&url, &dir) {
            Ok(local) => local,
            Err(message) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(CommandError::new(
                    "archive",
                    format!("无法读取归档「{title}」：{message}"),
                ));
            }
        };
        let owned = if local.starts_with(&dir) {
            Some(dir)
        } else {
            let _ = std::fs::remove_dir_all(&dir);
            None
        };
        let size = size.max(std::fs::metadata(&local).map(|meta| meta.len()).unwrap_or(0));

        let attempt = backend::list_entries(
            &local,
            (!password.is_empty()).then_some(password.as_str()),
        );
        match attempt {
            Ok(entries) => {
                self.finish_open(title, url, local, owned, size, entries, password);
                Ok(())
            }
            Err(message) if archive::password_error(&message) => {
                self.pending = Some(PendingArchive {
                    title: title.clone(),
                    source_url: url,
                    local,
                    owned,
                    size,
                });
                self.state.archive_locked = true;
                self.state.archive_lock_text =
                    format!("「{title}」已加密：输入归档密码，或从密码簿里选一条");
                self.state.status = format!("「{title}」已加密：等待密码");
                Ok(())
            }
            Err(message) => {
                if let Some(dir) = owned {
                    let _ = std::fs::remove_dir_all(dir);
                }
                self.sync_archive();
                Err(CommandError::new("archive", message))
            }
        }
    }

    fn finish_open(
        &mut self,
        title: String,
        source_url: String,
        local: PathBuf,
        owned: Option<PathBuf>,
        size: u64,
        entries: Vec<backend::ListedEntry>,
        password: String,
    ) {
        let encrypted = !password.is_empty() || archive::is_encrypted(&entries);
        let total = entries.len();
        self.pending = None;
        self.archive = Some(OpenArchive {
            title: title.clone(),
            local,
            owned,
            entries,
            prefix: Vec::new(),
            selected: BTreeSet::new(),
            sort: ArchiveSort::Name,
            descending: false,
            password: (!password.is_empty()).then_some(password),
            encrypted,
        });
        self.state.archive_locked = false;
        self.state.archive_lock_text.clear();
        self.sync_archive();
        self.recent = recent::touch(&title, &source_url, size, encrypted);
        self.sync_recent();
        self.state.status = format!("已打开「{title}」，共 {total} 项");
    }

    /// 删掉临时副本（复制出来的才删；桌面上打开的是原文件）。
    fn cleanup_archive(&mut self) {
        // 预览的 7z 进程还在读这份 staging：先停掉，再删目录。
        self.stop_preview();
        if let Some(archive) = self.archive.take()
            && let Some(dir) = archive.owned
        {
            let _ = std::fs::remove_dir_all(dir);
        }
        if let Some(pending) = self.pending.take()
            && let Some(dir) = pending.owned
        {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    fn layer_items(&self) -> Vec<archive::Item> {
        let Some(archive) = self.archive.as_ref() else {
            return Vec::new();
        };
        archive::layer(&archive.entries, &self.archive_prefix())
    }

    fn archive_prefix(&self) -> String {
        match self.archive.as_ref() {
            Some(archive) if !archive.prefix.is_empty() => {
                format!("{}/", archive.prefix.join("/"))
            }
            _ => String::new(),
        }
    }

    fn set_archive_sort(&mut self, by: ArchiveSort) {
        let Some(archive) = self.archive.as_mut() else {
            return;
        };
        if archive.sort == by {
            archive.descending = !archive.descending;
        } else {
            archive.sort = by;
            archive.descending = false;
        }
        self.sync_archive();
    }

    fn sync_archive(&mut self) {
        let Some(archive) = self.archive.as_ref() else {
            self.state.archive_open = false;
            self.state.archive_title.clear();
            self.state.archive_path.clear();
            self.state.archive_crumbs.clear();
            self.state.archive_entries.clear();
            self.state.archive_count = 0;
            self.state.archive_selected = 0;
            self.state.archive_selected_bytes = 0;
            self.state.archive_selected_text = "0 B".into();
            self.state.archive_empty_text.clear();
            self.state.archive_sort_text = "名称 ↑".into();
            self.state.archive_select_all_label = SELECT_ALL.into();
            self.state.archive_has_entries = false;
            self.state.archive_can_go_up = false;
            self.state.archive_by_name = true;
            self.state.archive_by_size = false;
            self.state.archive_descending = false;
            self.state.archive_encrypted = false;
            self.state.extract_label = "先选要解压的项".into();
            self.sync_extract();
            return;
        };
        let title = archive.title.clone();
        let prefix = archive.prefix.clone();
        let sort = archive.sort;
        let descending = archive.descending;
        let encrypted = archive.encrypted;
        let selected = archive.selected.clone();
        let flat_empty = archive.entries.is_empty();
        let mut items = archive::layer(&archive.entries, &self.archive_prefix());
        archive::sort(&mut items, sort, descending);

        self.state.archive_open = true;
        self.state.archive_title = title.clone();
        self.state.archive_path = if prefix.is_empty() {
            title.clone()
        } else {
            format!("{} / {}", title, prefix.join(" / "))
        };
        self.state.archive_crumbs = std::iter::once(lambda::ArchiveCrumb {
            id: 0,
            name: title,
        })
        .chain(prefix.iter().enumerate().map(|(index, name)| {
            lambda::ArchiveCrumb {
                id: index as i64 + 1,
                name: name.clone(),
            }
        }))
        .collect();
        self.state.archive_entries = items
            .iter()
            .map(|item| lambda::ArchiveEntry {
                id: item.id,
                name: item.name.clone(),
                size_text: if item.folder {
                    "—".into()
                } else {
                    browser::size_text(item.size)
                },
                detail_text: if item.folder {
                    "文件夹".into()
                } else if item.encrypted {
                    "加密文件".into()
                } else {
                    "文件".into()
                },
                mark: mark(selected.contains(&item.id)).into(),
                folder: item.folder,
                chosen: selected.contains(&item.id),
                encrypted: item.encrypted,
                preview: !item.folder && archive::is_text(&item.name),
                image: !item.folder && archive::is_image(&item.name),
            })
            .collect();
        self.state.archive_count = items.len() as i64;
        self.state.archive_has_entries = !items.is_empty();
        self.state.archive_empty_text = if !items.is_empty() {
            String::new()
        } else if flat_empty {
            "这个归档是空的".into()
        } else {
            "这一层没有内容".into()
        };
        self.state.archive_sort_text = format!(
            "{} {}",
            match sort {
                ArchiveSort::Name => "名称",
                ArchiveSort::Size => "大小",
            },
            if descending { "↓" } else { "↑" }
        );
        self.state.archive_can_go_up = !prefix.is_empty();
        self.state.archive_by_name = sort == ArchiveSort::Name;
        self.state.archive_by_size = sort == ArchiveSort::Size;
        self.state.archive_descending = descending;
        self.state.archive_encrypted = encrypted;
        self.apply_archive_selection();
    }

    fn apply_archive_selection(&mut self) {
        let items = self.layer_items();
        let Some(archive) = self.archive.as_mut() else {
            return;
        };
        archive
            .selected
            .retain(|id| items.iter().any(|item| item.id == *id));
        let selected = archive.selected.clone();
        let bytes: u64 = items
            .iter()
            .filter(|item| selected.contains(&item.id))
            .map(|item| item.size)
            .sum();
        self.state.archive_selected = selected.len() as i64;
        self.state.archive_selected_bytes = bytes as i64;
        self.state.archive_selected_text = browser::size_text(bytes);
        self.state.archive_select_all_label = if !items.is_empty()
            && selected.len() == items.len()
        {
            CLEAR_ALL.into()
        } else {
            SELECT_ALL.into()
        };
        self.state.extract_label = if selected.is_empty() {
            "先选要解压的项".into()
        } else {
            format!("解压 {} 项", selected.len())
        };
        self.sync_extract();
    }

    fn stop_preview(&mut self) {
        if let Some(session) = self.preview.take() {
            session.stop();
        }
        self.reset_preview_state();
    }

    fn reset_preview_state(&mut self) {
        self.state.preview_open = false;
        self.state.preview_title.clear();
        self.state.preview_text.clear();
        self.state.preview_page_text.clear();
        self.state.preview_page = 0;
        self.state.preview_loading = false;
        self.state.preview_has_prev = false;
        self.state.preview_has_next = false;
        self.state.preview_status.clear();
        self.state.preview_encoding.clear();
        self.state.preview_is_image = false;
        self.state.preview_image = MediaSource::new("");
    }

    /// 工作线程的页环 / 图片流状态搬进状态：文本只取当前页，图片只放流地址。
    fn sync_preview(&mut self) {
        let Some(session) = self.preview.as_ref() else {
            self.reset_preview_state();
            return;
        };
        match session {
            Preview::Text(session) => {
                let snapshot = session.snapshot();
                let total = browser::size_text(session.total_bytes);
                let title = session.title.clone();
                self.state.preview_open = true;
                self.state.preview_is_image = false;
                self.state.preview_image = MediaSource::new("");
                self.state.preview_title = title;
                self.state.preview_text = snapshot.text;
                self.state.preview_page = snapshot.page;
                self.state.preview_loading = snapshot.loading;
                self.state.preview_has_prev = snapshot.has_prev;
                self.state.preview_has_next = snapshot.has_next;
                self.state.preview_encoding = if snapshot.encoding.is_empty() {
                    "检测中".into()
                } else {
                    snapshot.encoding.clone()
                };
                self.state.preview_page_text = if snapshot.page <= 0 {
                    if snapshot.loading {
                        "读取中…".into()
                    } else {
                        "—".into()
                    }
                } else if snapshot.done {
                    format!("第 {} / {} 页", snapshot.page, snapshot.pages.max(snapshot.page))
                } else {
                    // 总页数要到读完才知道；加载中只报当前页，避免「第 1 / 4 页」
                    // 随预读增长而跳变。读取进度在下面的状态行。
                    format!("第 {} 页", snapshot.page)
                };
                self.state.preview_status = if let Some(error) = &snapshot.error {
                    format!("预览失败：{error}")
                } else if snapshot.done {
                    if snapshot.pages == 0 {
                        format!("空文件 · {} · {total}", self.state.preview_encoding)
                    } else {
                        format!(
                            "共 {} 页 · {} · {total}",
                            snapshot.pages, self.state.preview_encoding
                        )
                    }
                } else {
                    format!(
                        "读取中 · {} · 已读 {} / {total}",
                        self.state.preview_encoding,
                        browser::size_text(snapshot.loaded_bytes)
                    )
                };
            }
            Preview::Image(image) => {
                let snapshot = image.session.snapshot();
                self.state.preview_open = true;
                self.state.preview_is_image = true;
                self.state.preview_title = image.session.title().into();
                self.state.preview_image = image.session.url();
                self.state.preview_text.clear();
                self.state.preview_page = 0;
                self.state.preview_page_text = format!(
                    "第 {} / {} 张",
                    image.index + 1,
                    image.items.len().max(1)
                );
                self.state.preview_has_prev = image.index > 0;
                self.state.preview_has_next = image.index + 1 < image.items.len();
                self.state.preview_encoding.clear();
                self.state.preview_loading = snapshot.loading;
                self.state.preview_status = if let Some(error) = snapshot.error {
                    format!("预览失败：{error}")
                } else {
                    snapshot.detail
                };
            }
        }
    }

    fn sync_extract(&mut self) {
        let policy = self.state.extract_policy.clamp(0, 3);
        let at_root = self.stack.len() == 1;
        self.state.policy_current_ok = at_root;
        self.state.policy_text = match policy {
            0 => "当前文件夹",
            2 => "归档名文件夹",
            3 => "其他目录",
            _ => "智能解压",
        }
        .into();
        let current = self.stack.first().map(|step| step.name.clone());
        let (text, ready) = if policy == 3 {
            match &self.target {
                Some((_, label)) => (format!("解压到「{label}」的根目录"), true),
                None => ("还没有选择目标目录".into(), false),
            }
        } else if policy == 0 && !at_root {
            ("当前在子目录：SAF 只能写到目录根，换一种方式".into(), false)
        } else {
            match current {
                Some(label) => (format!("解压到「{label}」的根目录"), true),
                None => ("还没有目录：解压结果没有落点".into(), false),
            }
        };
        self.state.extract_target_text = text;
        self.state.extract_has_target = ready;
        self.state.can_extract = self.archive.is_some()
            && self.job.is_none()
            && ready
            && (policy != 0 || at_root);
    }

    fn sync_recent(&mut self) {
        self.state.recent = self
            .recent
            .iter()
            .map(|record| lambda::RecentArchive {
                id: record.id,
                name: record.name.clone(),
                detail_text: record.detail_text(),
                size_text: browser::size_text(record.size),
                encrypted: record.encrypted,
            })
            .collect();
        self.state.has_recent = !self.recent.is_empty();
    }

    fn sync_passwords(&mut self) {
        self.state.passwords = self
            .passwords
            .iter()
            .map(|record| lambda::SavedPassword {
                id: record.id.clone(),
                label: record.label.clone(),
                note: record.note(),
            })
            .collect();
        self.state.has_passwords = !self.passwords.is_empty();
    }

    /// 密码簿 id 一旦不可用就报错，而不是悄悄退回手输：用户选的是「这条
    /// 保存的密码」，用别的密码去开会让失败原因对不上。
    fn solve_password(
        &self,
        typed: String,
        secret: String,
    ) -> Result<String, CommandError> {
        if secret.is_empty() {
            return Ok(typed);
        }
        match passwords::value(&secret)? {
            Some(value) if !value.is_empty() => Ok(value),
            _ => Err(CommandError::new(
                "password",
                "这条保存的密码已经不可用：去密码簿重新保存",
            )),
        }
    }

    fn compress_selected(
        &mut self,
        name: Option<String>,
        password: String,
        secret: String,
    ) -> Result<(), CommandError> {
        if self.job.is_some() {
            return Err(CommandError::new(
                "job_busy",
                "已有任务在运行：等它结束，或先取消",
            ));
        }
        let Some(root) = self.stack.first().map(|step| step.url.clone()) else {
            return Err(CommandError::new(
                "job",
                "先选一个目录：压缩结果要落到目录的根目录",
            ));
        };
        let picked: Vec<compress::Input> = self
            .items
            .iter()
            .filter(|item| self.selected_ids.contains(&item.id))
            .map(|item| compress::Input {
                name: item.name.clone(),
                url: item.url.clone(),
                folder: item.folder,
            })
            .collect();
        if picked.is_empty() {
            return Err(CommandError::new("job", "先勾选要压缩的文件或文件夹"));
        }
        let password = self.solve_password(password, secret)?;
        let kind = if self.state.format_7z { "7z" } else { "zip" };
        let name = match name {
            Some(name) => {
                let trimmed = name.trim();
                let lower = trimmed.to_ascii_lowercase();
                let stem = if lower.ends_with(".7z") {
                    &trimmed[..trimmed.len() - 3]
                } else if lower.ends_with(".zip") {
                    &trimmed[..trimmed.len() - 4]
                } else {
                    trimmed
                };
                let clean = backend::file_component(stem);
                if clean == "archive" {
                    archive_stem(&picked)
                } else {
                    clean
                }
            }
            None => archive_stem(&picked),
        };
        let plan = compress::Plan {
            kind,
            level: self.state.level.clamp(0, 9) as u32,
            password,
            root,
            name,
        };
        self.spawn(picked, plan)
    }

    fn spawn_extract(
        &mut self,
        plan: extract::Plan,
        total: i64,
        target_label: &str,
    ) -> Result<(), CommandError> {
        // 7-Zip 调用串行：预览还开着就先收掉，避免任务排在它后面。
        self.stop_preview();
        let Some(waker) = bern::wake::waker() else {
            return Err(CommandError::new("job", "引擎没有 waker，任务无法在后台运行"));
        };
        let title = format!("解压 {total} 项");
        let shared = std::sync::Arc::new(std::sync::Mutex::new(Job::new(title.clone(), total)));
        self.job = Some(shared.clone());
        self.job_is_extract = true;
        self.state.job_active = true;
        self.state.job_title = title;
        self.state.job_phase = "准备".into();
        self.state.job_label = String::new();
        self.state.job_done = 0;
        self.state.job_total = total;
        self.state.job_percent = 0;
        self.state.job_percent_text = "准备中".into();
        self.state.job_cancellable = !cfg!(target_os = "android");
        self.state.can_compress = false;
        self.state.can_extract = false;
        self.state.status = format!("开始解压 {total} 项 → 「{target_label}」");
        extract::start(shared, waker, plan);
        Ok(())
    }

    /// 真正起任务：落记录、建 Job、交给工作线程，然后立刻返回。
    ///
    /// `compress` 与 `retry_leftover` 都走这里——两条路的差别只在「输入与
    /// 参数从哪来」，状态迁移必须完全一样。
    fn spawn(
        &mut self,
        inputs: Vec<compress::Input>,
        plan: compress::Plan,
    ) -> Result<(), CommandError> {
        self.stop_preview();
        let Some(waker) = bern::wake::waker() else {
            return Err(CommandError::new("job", "引擎没有 waker，任务无法在后台运行"));
        };
        let total = inputs.len() as i64;
        let title = format!("压缩 {total} 项");
        // 先落盘再起线程：反过来的话，进程在两步之间被杀就什么都不剩。
        record::begin(&record::Saved::new(
            &title,
            plan.kind,
            plan.level,
            &plan.name,
            &plan.root,
            !plan.password.is_empty(),
            inputs
                .iter()
                .map(|input| record::SavedInput {
                    name: input.name.clone(),
                    url: input.url.clone(),
                    folder: input.folder,
                })
                .collect(),
        ));

        let shared = std::sync::Arc::new(std::sync::Mutex::new(Job::new(title.clone(), total)));
        self.job = Some(shared.clone());
        self.job_is_extract = false;
        self.state.job_active = true;
        self.state.job_title = title;
        self.state.job_phase = "准备".into();
        self.state.job_label = String::new();
        self.state.job_done = 0;
        self.state.job_total = total;
        self.state.job_percent = 0;
        self.state.job_percent_text = "准备中".into();
        self.state.job_cancellable = !cfg!(target_os = "android");
        self.state.can_compress = false;
        self.state.can_extract = false;
        self.state.status = format!("开始压缩 {total} 项（{}）", plan.kind);
        // 新任务起来了，旧记录就该让位（重试时尤其要注意）。
        self.leftover = None;
        self.state.has_leftover = false;
        self.state.leftover_text.clear();

        compress::start(shared, waker, inputs, plan);
        Ok(())
    }

    /// 进一个位置的根。失败时标记失效，但不清空已有的浏览状态。
    fn enter_location(&mut self, record: &Record) -> Result<(), CommandError> {
        self.stack = vec![Step {
            name: record.label.clone(),
            url: record.uri.clone(),
        }];
        self.list_current()
    }

    /// 列出栈顶目录。错误分两类：授权失效（标记位置）与其它（只报告）。
    fn list_current(&mut self) -> Result<(), CommandError> {
        let Some(top) = self.stack.last() else {
            self.items.clear();
            self.rebuild();
            return Ok(());
        };
        let url = top.url.clone();
        let name = top.name.clone();
        self.state.busy = true;
        let outcome = browser::list(&url);
        self.state.busy = false;
        match outcome {
            Ok(items) => {
                self.items = items;
                // 选择是按目录算的：换一页就不该带着上一页的 id。
                self.selected_ids.clear();
                if let Some(id) = self.current_location() {
                    self.stale.remove(&id);
                }
                // 列表本身已经显示「共 N 项」；成功列举不再往状态行写一句话，
                // 否则它会跟着状态栏出现在每一页底部。任务在跑时也保留任务
                // 描述：用户刚点了刷新，但更想知道压缩还在不在。
                if self.job.is_none() {
                    self.state.status.clear();
                }
                self.rebuild();
                Ok(())
            }
            Err(error) => {
                self.items.clear();
                let folder_gone = error.code != "not_a_folder";
                if folder_gone && let Some(id) = self.current_location() {
                    self.stale.insert(id);
                }
                self.state.status = format!("无法读取「{name}」：{}", error.message);
                self.rebuild();
                Err(CommandError::new("browse", error.message))
            }
        }
    }

    /// 「全选」/「全不选」：当前页是否已经全选中。
    fn select_all_text(&self) -> String {
        let all = !self.items.is_empty() && self.selected_ids.len() == self.items.len();
        if all { CLEAR_ALL.into() } else { SELECT_ALL.into() }
    }

    /// 把选中的 id 投影成计数与合计字节；已经不存在的 id 直接丢掉。
    fn apply_selection(&mut self) {
        self.selected_ids
            .retain(|id| self.items.iter().any(|item| item.id == *id));
        self.state.selected = self.selected_ids.len() as i64;
        self.state.selected_bytes = self
            .items
            .iter()
            .filter(|item| self.selected_ids.contains(&item.id))
            .map(|item| item.size as i64)
            .sum();
        self.state.selected_text = browser::size_text(self.state.selected_bytes.max(0) as u64);
        // 压缩按钮的文案跟着选中数走：没选任何东西时它该说的是「先选文件」，
        // 而不是一个点了报错的「压缩 0 项」。
        self.state.compress_label = if self.state.selected > 0 {
            format!("压缩 {} 项", self.state.selected)
        } else {
            // 短一点：这一行还要放格式与级别两个按钮，360 dp 下长文案会把
            // 「压缩」挤到下一行（2026-11-06 真机）。
            "先选文件".into()
        };
        self.state.can_compress = self.state.selected > 0 && self.job.is_none();
    }

    /// 换格式：7z 能连文件名一起加密，zip 走 AES-256（文件名仍可见）。
    fn set_format(&mut self, seven: bool) {
        self.state.format_7z = seven;
        self.state.format_zip = !seven;
        self.state.format_text = if seven { "7z".into() } else { "zip".into() };
        self.state.password_note = password_note(seven);
        self.state.status = if seven {
            "格式：7z（填密码则连文件名一起加密）".into()
        } else {
            "格式：zip（AES-256 加密，文件名仍可见）".into()
        };
    }

    /// 「输出到「lambda-m0」的根目录」：SAF 只能写树根，界面必须写清楚，
    /// 否则用户会以为压缩包会落在当前这个子目录里。
    fn sync_output_note(&mut self) {
        self.state.output_note = match self.stack.first() {
            Some(step) => format!("输出 → 「{}」/", step.name),
            None => "还没有目录：压缩结果没有落点".into(),
        };
    }

    fn set_sort(&mut self, by: Sort) {
        if self.sort == by {
            self.descending = !self.descending;
        } else {
            self.sort = by;
            self.descending = false;
        }
        self.rebuild();
    }

    /// 把 `items` 投影成 state：排序、格式化、面包屑、位置条。
    fn rebuild(&mut self) {
        browser::sort(&mut self.items, self.sort, self.descending);
        self.state.entries = self
            .items
            .iter()
            .map(|item| lambda::Entry {
                id: item.id,
                name: item.name.clone(),
                chosen: self.selected_ids.contains(&item.id),
                mark: mark(self.selected_ids.contains(&item.id)).into(),
                size_text: if item.folder {
                    "—".into()
                } else {
                    browser::size_text(item.size)
                },
                detail_text: format!(
                    "{} · {}",
                    browser::kind_text(item),
                    browser::modified_text(item.modified)
                ),
                folder: item.folder,
                archive: item.archive,
            })
            .collect();
        self.state.count = self.items.len() as i64;
        self.state.has_entries = !self.items.is_empty();
        // 空表要有话说：真机上删掉目录后只剩表头，什么都点不动也没解释。
        self.state.empty_text = if !self.items.is_empty() {
            String::new()
        } else if self.current_location().is_some_and(|id| self.stale.contains(&id)) {
            "目录授权已失效：点上面那个 ↻ 重新授权".into()
        } else if self.table.is_empty() {
            "还没有目录：点「＋ 目录」选一个文件夹".into()
        } else {
            "这个文件夹是空的".into()
        };
        self.state.crumbs = self
            .stack
            .iter()
            .enumerate()
            .map(|(index, step)| lambda::Crumb {
                id: index as i64,
                name: step.name.clone(),
            })
            .collect();
        self.state.path = self
            .stack
            .iter()
            .map(|step| step.name.as_str())
            .collect::<Vec<_>>()
            .join(" / ");
        self.state.can_go_up = self.stack.len() > 1;
        self.state.sort_text = format!(
            "{} {}",
            match self.sort {
                Sort::Name => "名称",
                Sort::Size => "大小",
                Sort::Time => "修改时间",
            },
            if self.descending { "↓" } else { "↑" }
        );
        self.state.select_all_label = self.select_all_text();
        self.state.by_name = self.sort == Sort::Name;
        self.state.by_size = self.sort == Sort::Size;
        self.state.by_time = self.sort == Sort::Time;
        self.state.descending = self.descending;
        // 排序不改变 id，但换成另一个目录后旧的选中项就该消失。
        self.apply_selection();
        self.sync_output_note();
        self.sync_locations();
        // 换目录会改变「当前文件夹」是否可用（SAF 只能写树根）。
        self.sync_extract();
    }

    /// 目录条：可用 / 失效 / 当前。目录名不带「当前」标记，界面自己决定
    /// 怎么表达选中状态。
    fn sync_locations(&mut self) {
        let current = self.current_location();
        self.state.locations = self
            .table
            .iter()
            .map(|record| lambda::Location {
                id: record.id,
                label: record.label.clone(),
                note: if self.stale.contains(&record.id) {
                    "授权已失效".into()
                } else {
                    "可用".into()
                },
                stale: self.stale.contains(&record.id),
                current: current == Some(record.id),
            })
            .collect();
        self.state.has_locations = !self.table.is_empty();
        self.state.needs_location = self.table.is_empty();
    }

    /// 当前所在的**位置**（栈底那一节的 URI 对应哪条记录）。
    fn current_location(&self) -> Option<i64> {
        let root = self.stack.first()?;
        self.table
            .iter()
            .find(|record| record.uri == root.url)
            .map(|record| record.id)
    }
}

/// 行首那个按钮的字形。用文本而不是图标：`✓`/`○` 在任何字体下都在，
/// SF Symbols 的名字在 Android 上要靠一张映射表，缺一个就什么都不画。
fn mark(chosen: bool) -> &'static str {
    if chosen { "✓" } else { "○" }
}

/// 临时工作目录的唯一后缀：毫秒时间戳。归档浏览与任务串行，够用。
fn stamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0)
}

/// `-mx` 的档位名。0 是「仅存储」，7–9 都算极限——界面只给三档，
/// 这里仍按值映射，CLI 传任意 0–9 也能显示成一句人话。
fn level_text(level: i64) -> String {
    match level {
        0 => "仅存储".into(),
        1..=3 => "快速".into(),
        4..=6 => "标准".into(),
        7..=8 => "较高".into(),
        _ => "极限".into(),
    }
}

/// 密码对两种格式意味着不同的事，界面要照实说。
fn password_note(seven: bool) -> String {
    if seven {
        "7z 会连文件名一起加密".into()
    } else {
        "zip 用 AES-256，文件名可见".into()
    }
}

/// 归档名：选了一项就用它的名字（去掉扩展名），多项就是「压缩包」。
/// 冲突由导出端处理——SAF 会编号成 `压缩包 (1).7z`。
fn archive_stem(inputs: &[compress::Input]) -> String {
    if inputs.len() == 1 {
        let name = &inputs[0].name;
        let stem = std::path::Path::new(name)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.is_empty())
            .unwrap_or(name);
        return backend::file_component(stem);
    }
    "压缩包".into()
}

impl lambda::Engine for LambdaEngine {
    fn state(&self) -> &State {
        &self.state
    }

    /// 工作线程的进度搬进状态。宿主随后发布状态，界面才看得到变化。
    ///
    /// `Waker::wake` 自带合并，工作线程可以放心地每 100 ms 调一次；这里
    /// 只做一次快照拷贝，不碰磁盘。只有任务真的结束（`outcome` 有值）时
    /// 才刷新目录——产物已经在位置的根上，刷新是为了让它出现在列表里。
    fn wake(&mut self) {
        self.open_handed_files();
        self.sync_job();
        if self.preview.is_some() {
            self.sync_preview();
        }
    }
}

impl LambdaEngine {
    /// 系统用 Lambda 打开的文件：双击、文件管理器的“打开方式”、拖到 exe 上。
    /// 一次交来多个时依次打开，最后一个是眼前看到的那个。
    fn open_handed_files(&mut self) {
        for url in files::take_opened() {
            if let Err(error) = self.open_file(MediaSource::new(url), String::new(), String::new())
            {
                self.state.status = error.message;
            }
        }
    }

    /// 任务进度搬进状态；没有任务时什么都不做。预览的 wake 也走 `Engine::wake`，
    /// 所以这里不能提前把整个 wake 返回掉。
    fn sync_job(&mut self) {
        let Some(shared) = self.job.clone() else {
            return;
        };
        let snapshot = match shared.lock() {
            Ok(job) => (
                job.title.clone(),
                job.phase.clone(),
                job.label.clone(),
                job.done,
                job.total,
                job.percent(),
                job.percent_text(),
                job.outcome.clone(),
            ),
            Err(_) => return,
        };
        let (title, phase, label, done, total, percent, percent_text, outcome) = snapshot;
        self.state.job_active = true;
        self.state.job_title = title;
        self.state.job_phase = phase;
        self.state.job_label = label;
        self.state.job_done = done;
        self.state.job_total = total;
        self.state.job_percent = percent.clamp(0, 100);
        self.state.job_percent_text = percent_text;
        // 有任务在跑就不能再点「压缩」：按钮的可用性只能由状态给出。
        self.state.can_compress = self.job.is_none();

        let Some(outcome) = outcome else {
            return;
        };
        let was_extract = self.job_is_extract;
        self.job = None;
        self.job_is_extract = false;
        // 解压不写任务记录；这里若一律 `finish()`，会把一条还没处理的
        // 「上次压缩未完成」记录一起删掉。
        if !was_extract {
            record::finish();
        }
        self.state.job_active = false;
        self.state.job_done = self.state.job_total;
        self.state.job_percent = 100;
        self.state.job_percent_text = "100%".into();
        self.state.can_compress = self.state.selected > 0;
        let (message, refresh) = match outcome {
            Outcome::Done(message) => (message, true),
            Outcome::Failed(message) => {
                let prefix = if was_extract { "解压失败" } else { "压缩失败" };
                (format!("{prefix}：{message}"), true)
            }
            Outcome::Cancelled if was_extract => {
                ("已取消：解压出的临时文件已清理，没有导出".into(), false)
            }
            Outcome::Cancelled => ("已取消：没有生成归档".into(), false),
        };
        if refresh {
            // 先刷新再写消息：`refresh` 成功时会把状态改成「「x」共 N 项」，
            // 反过来就把这次的结果盖掉了。
            let _ = self.list_current();
        }
        self.sync_extract();
        self.state.status = message;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lambda::Commands;
    use lambda::Engine;
    use std::time::UNIX_EPOCH;

    fn item(name: &str, folder: bool, size: u64) -> Item {
        Item {
            id: locations::id_of(name),
            name: name.into(),
            url: format!("content://tree/x/document/{name}"),
            folder,
            archive: browser::is_archive(name),
            size,
            modified: Some(UNIX_EPOCH),
        }
    }

    /// 不碰磁盘的引擎：位置表与条目都由测试给。
    fn engine(items: Vec<Item>) -> LambdaEngine {
        let mut state = base_state();
        state.engine = "test".into();
        state.status = String::new();
        let mut engine = LambdaEngine {
            state,
            table: vec![Record {
                id: locations::id_of("content://tree/x"),
                label: "测试".into(),
                uri: "content://tree/x".into(),
                added_at: 0,
            }],
            items,
            stack: vec![Step {
                name: "测试".into(),
                url: "content://tree/x".into(),
            }],
            stale: BTreeSet::new(),
            selected_ids: BTreeSet::new(),
            sort: Sort::Name,
            descending: false,
            job: None,
            job_is_extract: false,
            leftover: None,
            archive: None,
            pending: None,
            target: None,
            recent: Vec::new(),
            passwords: Vec::new(),
            preview: None,
        };
        engine.rebuild();
        engine
    }

    #[test]
    fn entries_carry_no_urls() {
        // 一万个条目的状态下，URL 不该出现在 state 里——界面一个字都用不上。
        let engine = engine(vec![item("a.txt", false, 1)]);
        let entry = &engine.state.entries[0];
        assert_eq!(entry.name, "a.txt");
        assert!(entry.detail_text.starts_with("文件 ·"));
        assert!(!entry.size_text.contains("content:"));
    }

    #[test]
    fn folders_get_a_dash_instead_of_a_size() {
        let engine = engine(vec![item("dir", true, 0)]);
        assert_eq!(engine.state.entries[0].size_text, "—");
        assert!(engine.state.entries[0].detail_text.starts_with("文件夹 ·"));
    }

    #[test]
    fn toggling_rows_adds_up_bytes_and_marks_the_row() {
        let mut engine = engine(vec![
            item("a.bin", false, 100),
            item("b.bin", false, 200),
            item("c.bin", false, 300),
        ]);
        let a = locations::id_of("a.bin");
        let c = locations::id_of("c.bin");
        engine.toggle(a).unwrap();
        engine.toggle(c).unwrap();
        assert_eq!(engine.state.selected, 2);
        assert_eq!(engine.state.selected_bytes, 400);
        let marks: Vec<&str> = engine.state.entries.iter().map(|entry| entry.mark.as_str()).collect();
        assert_eq!(marks, ["✓", "○", "✓"]);
        engine.toggle(a).unwrap();
        assert_eq!(engine.state.selected, 1);
        assert_eq!(engine.state.selected_bytes, 300);
    }

    #[test]
    fn select_all_flips_both_ways() {
        let mut engine = engine(vec![
            item("a.bin", false, 10),
            item("b.bin", false, 20),
        ]);
        assert_eq!(engine.state.select_all_label, SELECT_ALL);
        engine.select_all().unwrap();
        assert_eq!(engine.state.selected, 2);
        assert_eq!(engine.state.selected_bytes, 30);
        assert_eq!(engine.state.select_all_label, CLEAR_ALL);
        engine.select_all().unwrap();
        assert_eq!(engine.state.selected, 0);
        assert_eq!(engine.state.select_all_label, SELECT_ALL);
    }

    #[test]
    fn opening_a_file_only_reports() {
        let mut engine = engine(vec![item("a.txt", false, 1)]);
        engine.open(locations::id_of("a.txt")).unwrap();
        assert!(engine.state.status.contains("不打开普通文件"));
    }

    #[test]
    fn opening_an_archive_enters_the_archive_browser() {
        let mut engine = engine(vec![item("a.7z", false, 1)]);
        let error = engine.open(locations::id_of("a.7z")).unwrap_err();
        // 测试里的 URL 不是真文档：复制失败是预期结局，但错误必须来自
        // 归档打开流程，而不是「M4 还没接」。
        assert_eq!(error.code, "archive");
        assert!(error.message.contains("无法读取归档"), "{}", error.message);
    }

    #[test]
    fn a_picked_file_goes_through_the_archive_flow() {
        let dir = std::env::temp_dir().join(format!("lambda-picked-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.7z");
        std::fs::write(&path, b"not an archive").unwrap();
        let mut engine = engine(Vec::new());
        let error = engine
            .open_file(MediaSource::file(&path), String::new(), String::new())
            .unwrap_err();
        assert_eq!(error.code, "archive");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_the_system_hands_over_is_taken_on_wake() {
        let dir = std::env::temp_dir().join(format!("lambda-handed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("handed.7z");
        std::fs::write(&path, b"not an archive").unwrap();
        files::opened(&MediaSource::file(&path).url);
        let mut engine = engine(Vec::new());
        engine.wake();
        // 坏归档：打开失败也要留下消息，不能悄悄吞掉系统交来的文件。
        assert!(!engine.state.status.is_empty(), "{}", engine.state.status);
        assert!(engine.archive.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clear_keeps_the_location_table() {
        let mut engine = engine(vec![item("a.txt", false, 1)]);
        engine.state.selected = 3;
        engine.clear().unwrap();
        assert_eq!(engine.state.selected, 0);
        assert!(engine.state.status.contains("准备就绪"));
        assert_eq!(engine.table.len(), 1);
    }

    #[test]
    fn the_selected_total_is_formatted_for_reading() {
        // 真机上「1757282 字节」看不出量级；底部要的是「1.7 MB」。
        let mut engine = engine(vec![
            item("a.bin", false, 1_000_000),
            item("b.bin", false, 757_282),
        ]);
        engine.select_all().unwrap();
        assert_eq!(engine.state.selected_bytes, 1_757_282);
        assert_eq!(engine.state.selected_text, "1.7 MB");
        engine.select_all().unwrap();
        assert_eq!(engine.state.selected_text, "0 B");
    }

    #[test]
    fn an_empty_page_says_why() {
        let empty = engine(Vec::new());
        assert!(!empty.state.has_entries);
        assert_eq!(empty.state.empty_text, "这个文件夹是空的");
        let full = engine(vec![item("a.txt", false, 1)]);
        assert!(full.state.empty_text.is_empty());
    }

    #[test]
    fn a_broken_location_explains_the_empty_page() {
        let mut engine = engine(Vec::new());
        let id = engine.current_location().unwrap();
        engine.stale.insert(id);
        engine.rebuild();
        assert!(engine.state.empty_text.contains("授权已失效"));
        assert!(engine.state.empty_text.contains("↻"));
    }

    #[test]
    fn the_current_location_is_marked_without_changing_its_name() {
        let engine = engine(vec![item("a.txt", false, 1)]);
        assert_eq!(engine.state.locations[0].label, "测试");
        assert!(engine.state.locations[0].current);
    }

    #[test]
    fn selecting_all_leaves_the_status_line_alone() {
        // 状态行是目录描述（「「many」共 10001 项」），不该被选择数顶掉。
        let mut engine = engine(vec![item("a.txt", false, 1)]);
        engine.state.status = "「测试」共 1 项".into();
        engine.select_all().unwrap();
        assert_eq!(engine.state.status, "「测试」共 1 项");
    }
}
