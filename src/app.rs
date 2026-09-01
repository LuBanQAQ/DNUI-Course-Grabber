use std::{
    collections::BTreeSet,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use chrono::{Local, NaiveTime, TimeZone};
use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use serde_json::Value;

use crate::{
    api::{ApiClient, LoginSession, SessionExpired},
    model::{Batch, Course, Prefs},
    network::{VPN_GUIDANCE, VPN_PORTAL, VpnRequired},
};

#[allow(clippy::large_enum_variant)]
enum WorkerEvent {
    Status(String),
    NetworkReady,
    VpnAuthorized,
    VpnRequired {
        task_finished: bool,
        detail: Option<String>,
    },
    LoginReady(LoginSession),
    SessionRefreshed(LoginSession),
    Courses {
        session: LoginSession,
        batch: Batch,
        teaching_class_type: String,
        campus: String,
        courses: Vec<Course>,
        total: usize,
    },
    RobFinished {
        successful: Vec<String>,
        pending: Vec<String>,
        failure: Option<String>,
    },
    Error(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ControlPage {
    Account,
    Courses,
    Task,
    Advanced,
}

#[derive(Clone)]
struct AutoRelogin {
    enabled: bool,
    used: Arc<AtomicBool>,
    gate: Arc<Mutex<()>>,
    session: Arc<Mutex<LoginSession>>,
    username: String,
    password: String,
    batch: Batch,
    tx: Sender<WorkerEvent>,
}

impl AutoRelogin {
    fn current_session(&self) -> anyhow::Result<LoginSession> {
        self.session
            .lock()
            .map(|session| session.clone())
            .map_err(|_| anyhow::anyhow!("登录会话状态异常"))
    }

    fn recover(&self) -> anyhow::Result<bool> {
        if !self.enabled {
            return Ok(false);
        }
        let _guard = self
            .gate
            .lock()
            .map_err(|_| anyhow::anyhow!("自动登录锁异常"))?;
        if self.used.swap(true, Ordering::SeqCst) {
            // Another worker (usually heartbeat) may have just refreshed the shared session.
            // Re-run the operation with that session, but never perform a second login.
            return Ok(true);
        }
        let _ = self.tx.send(WorkerEvent::Status(
            "检测到登录会话失效，正在自动重新登录（本任务最多一次）...".to_owned(),
        ));
        let session = ApiClient::login(&self.username, &self.password, 5)
            .map_err(|error| error.context("掉线自动登录失败"))?;
        session
            .api
            .bind_batch(&session.token, &self.batch.code)
            .map_err(|error| error.context("自动登录后重新进入课程轮次失败"))?;
        *self
            .session
            .lock()
            .map_err(|_| anyhow::anyhow!("登录会话状态异常"))? = session.clone();
        let _ = self.tx.send(WorkerEvent::SessionRefreshed(session.clone()));
        let _ = self.tx.send(WorkerEvent::Status(
            "自动重新登录成功，已恢复当前轮次并继续任务".to_owned(),
        ));
        Ok(true)
    }
}

fn with_auto_relogin<T>(
    relogin: &AutoRelogin,
    mut operation: impl FnMut(&LoginSession) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let session = relogin.current_session()?;
    match operation(&session) {
        Err(error) if error.is::<SessionExpired>() => {
            if relogin.recover()? {
                operation(&relogin.current_session()?)
            } else {
                Err(error)
            }
        }
        result => result,
    }
}

impl WorkerEvent {
    fn from_error(error: anyhow::Error) -> Self {
        if error.is::<VpnRequired>() {
            Self::VpnRequired {
                task_finished: false,
                detail: error.chain().nth(1).map(|_| error.to_string()),
            }
        } else {
            Self::Error(format!("{error:#}"))
        }
    }
}

pub struct XkApp {
    prefs: Prefs,
    prefs_path: PathBuf,
    status: String,
    busy: bool,
    tx: Sender<WorkerEvent>,
    rx: Receiver<WorkerEvent>,
    login_session: Option<LoginSession>,
    batches: Vec<Batch>,
    batch_dialog: bool,
    selected_batch: usize,
    current_batch: Option<Batch>,
    teaching_class_type: String,
    campus: String,
    courses: Vec<Course>,
    total: usize,
    selected_courses: BTreeSet<usize>,
    filter: String,
    rob_running: bool,
    paused: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    result_dialog: Option<String>,
    vpn_required: bool,
    control_page: ControlPage,
}

impl XkApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_chinese_font(&cc.egui_ctx);
        cc.egui_ctx.set_theme(egui::Theme::Light);
        let mut visuals = egui::Visuals::light();
        visuals.panel_fill = egui::Color32::from_rgb(242, 247, 253);
        visuals.window_fill = egui::Color32::from_rgb(250, 252, 255);
        visuals.extreme_bg_color = egui::Color32::WHITE;
        visuals.faint_bg_color = egui::Color32::from_rgb(232, 240, 250);
        visuals.selection.bg_fill = egui::Color32::from_rgb(52, 120, 246);
        visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
        visuals.hyperlink_color = egui::Color32::from_rgb(31, 91, 190);
        visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(239, 244, 251);
        visuals.widgets.inactive.bg_stroke =
            egui::Stroke::new(1.0, egui::Color32::from_rgb(193, 207, 225));
        visuals.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(220, 233, 250);
        visuals.widgets.active.weak_bg_fill = egui::Color32::from_rgb(200, 220, 247);
        cc.egui_ctx.set_visuals(visuals);
        let mut style = (*cc.egui_ctx.style_of(egui::Theme::Light)).clone();
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(14.0, 7.0);
        style.spacing.interact_size.y = 30.0;
        style.visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(5);
        style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(5);
        style.visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(5);
        style.visuals.widgets.active.corner_radius = egui::CornerRadius::same(5);
        cc.egui_ctx.set_style_of(egui::Theme::Light, style);
        let prefs_path = app_file("xk-rust-prefs.json");
        let prefs = fs::read_to_string(&prefs_path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_else(Prefs::with_defaults);
        let (tx, rx) = unbounded();
        Self {
            prefs,
            prefs_path,
            status: "请输入学号和密码".to_owned(),
            busy: false,
            tx,
            rx,
            login_session: None,
            batches: Vec::new(),
            batch_dialog: false,
            selected_batch: 0,
            current_batch: None,
            teaching_class_type: String::new(),
            campus: "1".to_owned(),
            courses: Vec::new(),
            total: 0,
            selected_courses: BTreeSet::new(),
            filter: String::new(),
            rob_running: false,
            paused: Arc::new(AtomicBool::new(false)),
            stopped: Arc::new(AtomicBool::new(false)),
            result_dialog: None,
            vpn_required: false,
            control_page: ControlPage::Account,
        }
    }

    fn save_prefs(&self) {
        if let Ok(text) = serde_json::to_string_pretty(&self.prefs) {
            let _ = fs::write(&self.prefs_path, text);
        }
    }

    fn login(&mut self) {
        let username = self.prefs.username.trim().to_owned();
        let password = self.prefs.password.clone();
        if username.is_empty() || password.is_empty() {
            self.result_dialog = Some("请输入学号和密码".to_owned());
            return;
        }
        self.save_prefs();
        self.busy = true;
        self.vpn_required = false;
        self.status = "正在检查校园网/VPN并登录...".to_owned();
        let tx = self.tx.clone();
        thread::spawn(move || match ApiClient::login(&username, &password, 5) {
            Ok(session) => {
                let _ = tx.send(WorkerEvent::LoginReady(session));
            }
            Err(error) => {
                let _ = tx.send(WorkerEvent::from_error(error));
            }
        });
    }

    fn check_network(&mut self) {
        self.busy = true;
        self.status = "正在检测本程序能否访问选课接口...".to_owned();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let event = match ApiClient::check_network() {
                Ok(()) => WorkerEvent::NetworkReady,
                Err(error) => WorkerEvent::from_error(error),
            };
            let _ = tx.send(event);
        });
    }

    fn authorize_vpn(&mut self) {
        self.busy = true;
        self.status = "请在独立授权窗口完成校园 VPN 认证，再点击“完成授权并检测”".to_owned();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let event = match ApiClient::authorize_vpn() {
                Ok(()) => WorkerEvent::VpnAuthorized,
                Err(error) => WorkerEvent::from_error(error),
            };
            let _ = tx.send(event);
        });
    }

    fn choose_batch(&mut self) {
        let Some(session) = self.login_session.clone() else {
            return;
        };
        let Some(batch) = self.batches.get(self.selected_batch).cloned() else {
            return;
        };
        if batch.can_select != "1" {
            self.result_dialog = Some(
                batch
                    .no_select_reason
                    .clone()
                    .unwrap_or_else(|| "该轮次不可选".to_owned()),
            );
            return;
        }
        self.batch_dialog = false;
        self.busy = true;
        self.status = format!("正在进入轮次：{}", batch.name);
        let tx = self.tx.clone();
        let interval = self.prefs.page_interval_ms;
        let retries = self.prefs.list_403_retry;
        let penalty_ms = self.prefs.penalty_ms;
        thread::spawn(move || {
            let result = (|| {
                let campus = session.api.bind_batch(&session.token, &batch.code)?;
                let class_type = session
                    .api
                    .discover_teaching_class_type(&session.token, &batch.code)?;
                let _ = tx.send(WorkerEvent::Status(format!(
                    "正在抓取课程列表（{class_type}）..."
                )));
                let (mut courses, total) = session.api.fetch_courses(
                    &session.token,
                    &batch.code,
                    &class_type,
                    &campus,
                    100,
                    interval,
                    retries,
                    penalty_ms,
                )?;
                match session.api.fetch_courses(
                    &session.token,
                    &batch.code,
                    "YXKC",
                    &campus,
                    100,
                    interval,
                    retries,
                    penalty_ms,
                ) {
                    Ok((mut enrolled, _)) => {
                        for course in &mut enrolled {
                            course.raw["_enrolled"] = Value::Bool(true);
                        }
                        let existing: BTreeSet<String> = courses.iter().map(Course::id).collect();
                        courses.extend(
                            enrolled
                                .into_iter()
                                .filter(|course| !existing.contains(&course.id())),
                        );
                    }
                    Err(error) if error.is::<VpnRequired>() => return Err(error),
                    Err(_) => {}
                }
                anyhow::Ok((class_type, campus, courses, total))
            })();
            match result {
                Ok((teaching_class_type, campus, courses, total)) => {
                    let _ = tx.send(WorkerEvent::Courses {
                        session,
                        batch,
                        teaching_class_type,
                        campus,
                        courses,
                        total,
                    });
                }
                Err(error) => {
                    let _ = tx.send(WorkerEvent::from_error(error));
                }
            }
        });
    }

    fn start_rob(&mut self) {
        let Some(session) = self.login_session.clone() else {
            self.result_dialog = Some("请先登录并抓取课程".to_owned());
            return;
        };
        let Some(batch) = self.current_batch.clone() else {
            return;
        };
        let courses: Vec<Course> = self
            .selected_courses
            .iter()
            .filter_map(|index| self.courses.get(*index).cloned())
            .collect();
        let select_any_available = self.prefs.select_any_available;
        if courses.is_empty() && !select_any_available {
            self.result_dialog = Some("请先勾选至少一门课程".to_owned());
            return;
        }
        let class_type = self.teaching_class_type.clone();
        let campus = self.campus.clone();
        let click_times = self.prefs.click_times.max(1);
        let click_interval = self.prefs.click_interval_ms;
        let list_retries = self.prefs.list_403_retry;
        let penalty_ms = self.prefs.penalty_ms;
        let keep_alive = self.prefs.keep_alive;
        let keep_alive_seconds = self.prefs.keep_alive_seconds.max(1);
        let schedule = self.prefs.scheduled_start.trim().to_owned();
        let relogin = AutoRelogin {
            enabled: self.prefs.auto_relogin,
            used: Arc::new(AtomicBool::new(false)),
            gate: Arc::new(Mutex::new(())),
            session: Arc::new(Mutex::new(session)),
            username: self.prefs.username.trim().to_owned(),
            password: self.prefs.password.clone(),
            batch: batch.clone(),
            tx: self.tx.clone(),
        };
        self.save_prefs();
        self.rob_running = true;
        self.control_page = ControlPage::Task;
        self.paused.store(false, Ordering::Relaxed);
        self.stopped.store(false, Ordering::Relaxed);
        let paused = self.paused.clone();
        let stopped = self.stopped.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let heartbeat_finished = Arc::new(AtomicBool::new(false));
            let heartbeat = if keep_alive {
                let heartbeat_relogin = relogin.clone();
                let heartbeat_stopped = stopped.clone();
                let heartbeat_finished = heartbeat_finished.clone();
                let heartbeat_tx = tx.clone();
                Some(thread::spawn(move || {
                    while !heartbeat_stopped.load(Ordering::Relaxed)
                        && !heartbeat_finished.load(Ordering::Relaxed)
                    {
                        let status = match with_auto_relogin(&heartbeat_relogin, |session| {
                            session.api.heartbeat(&session.token)
                        }) {
                            Ok(()) => "保活成功".to_owned(),
                            Err(error) if error.is::<VpnRequired>() => {
                                heartbeat_stopped.store(true, Ordering::Relaxed);
                                let _ = heartbeat_tx.send(WorkerEvent::from_error(error));
                                return;
                            }
                            Err(error) => format!("保活失败：{error:#}"),
                        };
                        let _ = heartbeat_tx.send(WorkerEvent::Status(status));

                        let deadline = Instant::now() + Duration::from_secs(keep_alive_seconds);
                        while Instant::now() < deadline
                            && !heartbeat_stopped.load(Ordering::Relaxed)
                            && !heartbeat_finished.load(Ordering::Relaxed)
                        {
                            thread::sleep(Duration::from_millis(100));
                        }
                    }
                }))
            } else {
                None
            };
            if !schedule.is_empty() {
                match wait_until(&schedule, &stopped, &tx) {
                    Ok(()) => {}
                    Err(error) => {
                        heartbeat_finished.store(true, Ordering::Relaxed);
                        if let Some(heartbeat) = heartbeat {
                            let _ = heartbeat.join();
                        }
                        let _ = tx.send(WorkerEvent::Error(error));
                        return;
                    }
                }
            }
            let mut pending = courses;
            let mut successful = Vec::new();
            let mut successful_ids = BTreeSet::new();
            let mut failure = None;
            let mut any_course_selected = false;
            for round in 1..=click_times {
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                while paused.load(Ordering::Relaxed) && !stopped.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(100));
                }
                if (!select_any_available && pending.is_empty()) || stopped.load(Ordering::Relaxed)
                {
                    break;
                }
                if select_any_available {
                    let refreshed = with_auto_relogin(&relogin, |session| {
                        session.api.fetch_available_courses(
                            &session.token,
                            &batch.code,
                            &class_type,
                            &campus,
                            100,
                            1_500,
                            list_retries,
                            penalty_ms,
                        )
                    });
                    match refreshed {
                        Ok((fresh, _)) => {
                            let ready = available_candidates(fresh, &successful_ids);
                            let _ = tx.send(WorkerEvent::Status(format!(
                                "任意未满课程查询 {round}/{click_times}：发现 {} 门可选课程",
                                ready.len()
                            )));
                            if ready.is_empty() {
                                pending.clear();
                                wait_interval(click_interval.max(500), &stopped);
                                continue;
                            }
                            pending = ready;
                        }
                        Err(error) if error.is::<SessionExpired>() => {
                            failure = Some(format!(
                                "登录会话已失效，且未能自动恢复：{error:#}\n任务已停止，请重新登录后再试。"
                            ));
                            stopped.store(true, Ordering::Relaxed);
                            break;
                        }
                        Err(error) => {
                            if error.is::<VpnRequired>() {
                                let _ = tx.send(WorkerEvent::from_error(error));
                                stopped.store(true, Ordering::Relaxed);
                            } else {
                                let _ = tx.send(WorkerEvent::Status(format!(
                                    "余量监控 {round}/{click_times} 刷新失败：{error:#}"
                                )));
                            }
                            wait_interval(click_interval.max(500), &stopped);
                            continue;
                        }
                    }
                }
                let mut next = Vec::new();
                for course in pending {
                    if stopped.load(Ordering::Relaxed) {
                        next.push(course);
                        continue;
                    }
                    while paused.load(Ordering::Relaxed) && !stopped.load(Ordering::Relaxed) {
                        thread::sleep(Duration::from_millis(100));
                    }
                    if stopped.load(Ordering::Relaxed) {
                        next.push(course);
                        continue;
                    }
                    let name = course.name();
                    if select_any_available && !course.has_available_seat() {
                        next.push(course);
                        continue;
                    }
                    match with_auto_relogin(&relogin, |session| {
                        session
                            .api
                            .select_course(&session.token, &batch.code, &class_type, &course)
                    }) {
                        Ok(body) => {
                            let code = body.get("code").and_then(Value::as_i64).unwrap_or_default();
                            let msg = body.get("msg").and_then(Value::as_str).unwrap_or("");
                            let _ = tx.send(WorkerEvent::Status(format!(
                                "轮次 {round}/{click_times} | {name}: code={code}, {msg}"
                            )));
                            if selection_succeeded(&body) {
                                successful_ids.insert(course.id());
                                successful.push(name);
                                if select_any_available {
                                    any_course_selected = true;
                                    break;
                                }
                            } else {
                                next.push(course);
                            }
                        }
                        Err(error) => {
                            failure = Some(stop_after_submission_error(&name, &error, &stopped));
                            if error.is::<VpnRequired>() {
                                let _ = tx.send(WorkerEvent::from_error(error));
                            }
                            next.push(course);
                        }
                    }
                    wait_interval(click_interval, &stopped);
                }
                pending = next;
                if any_course_selected {
                    break;
                }
            }
            heartbeat_finished.store(true, Ordering::Relaxed);
            if let Some(heartbeat) = heartbeat {
                let _ = heartbeat.join();
            }
            let pending_names = pending.iter().map(Course::name).collect();
            let _ = tx.send(WorkerEvent::RobFinished {
                successful,
                pending: pending_names,
                failure,
            });
        });
    }

    fn start_swap(&mut self) {
        let selected: Vec<Course> = self
            .selected_courses
            .iter()
            .filter_map(|index| self.courses.get(*index).cloned())
            .collect();
        if selected.len() != 2 {
            self.result_dialog = Some("换课模式请只勾选两门：当前已选 A 和目标 B".to_owned());
            return;
        }
        let Some(current) = selected.iter().find(|course| course.enrolled()).cloned() else {
            self.result_dialog =
                Some("两门课程中没有识别到当前已选课程 A（状态应显示“已选”）".to_owned());
            return;
        };
        let Some(target) = selected.iter().find(|course| !course.enrolled()).cloned() else {
            self.result_dialog = Some("两门课程中没有目标课程 B".to_owned());
            return;
        };
        let (Some(session), Some(batch)) = (self.login_session.clone(), self.current_batch.clone())
        else {
            return;
        };
        let class_type = self.teaching_class_type.clone();
        let campus = self.campus.clone();
        let rounds = self.prefs.click_times.max(1);
        let interval = self.prefs.click_interval_ms;
        let retries = self.prefs.list_403_retry;
        let penalty_ms = self.prefs.penalty_ms;
        let tx = self.tx.clone();
        let stopped = self.stopped.clone();
        let relogin = AutoRelogin {
            enabled: self.prefs.auto_relogin,
            used: Arc::new(AtomicBool::new(false)),
            gate: Arc::new(Mutex::new(())),
            session: Arc::new(Mutex::new(session)),
            username: self.prefs.username.trim().to_owned(),
            password: self.prefs.password.clone(),
            batch: batch.clone(),
            tx: self.tx.clone(),
        };
        self.save_prefs();
        self.rob_running = true;
        self.control_page = ControlPage::Task;
        self.stopped.store(false, Ordering::Relaxed);
        thread::spawn(move || {
            for round in 1..=rounds {
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                let fresh = with_auto_relogin(&relogin, |session| {
                    session.api.fetch_available_courses(
                        &session.token,
                        &batch.code,
                        &class_type,
                        &campus,
                        100,
                        1_500,
                        retries,
                        penalty_ms,
                    )
                });
                let (courses, _) = match fresh {
                    Ok(courses) => courses,
                    Err(error) if error.is::<VpnRequired>() => {
                        let _ = tx.send(WorkerEvent::VpnRequired {
                            task_finished: true,
                            detail: Some("VPN 验证失效，换课已停止；尚未提交退课请求。".to_owned()),
                        });
                        return;
                    }
                    Err(error) if error.is::<SessionExpired>() => {
                        let _ = tx.send(WorkerEvent::Error(format!(
                            "换课监控检测到登录会话失效，且未能自动恢复：{error:#}"
                        )));
                        return;
                    }
                    Err(error) => {
                        let _ = tx.send(WorkerEvent::Status(format!(
                            "换课轮次 {round}/{rounds}：刷新课程失败：{error:#}"
                        )));
                        wait_interval(interval.max(500), &stopped);
                        continue;
                    }
                };
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                let Some(fresh_target) = courses
                    .into_iter()
                    .find(|course| course.id() == target.id())
                else {
                    let _ = tx.send(WorkerEvent::Status(format!(
                        "未满课程查询 {round}/{rounds}：{} 暂无余量，保留 {}",
                        target.name(),
                        current.name()
                    )));
                    wait_interval(interval.max(500), &stopped);
                    continue;
                };
                if !fresh_target.has_available_seat() {
                    let _ = tx.send(WorkerEvent::Status(format!(
                        "换课轮次 {round}/{rounds}：{} 暂无余量，保留 {}",
                        target.name(),
                        current.name()
                    )));
                    wait_interval(interval.max(500), &stopped);
                    continue;
                }
                let _ = tx.send(WorkerEvent::Status(format!(
                    "{} 已有空位，正在退选 {}",
                    target.name(),
                    current.name()
                )));
                let drop_result = relogin.current_session().and_then(|session| {
                    session
                        .api
                        .drop_course(&session.token, &batch.code, &class_type, &current)
                });
                match drop_result {
                    Ok(body) if selection_succeeded(&body) => {}
                    Ok(body) => {
                        let _ = tx.send(WorkerEvent::Error(format!(
                            "退选 {} 失败：{}",
                            current.name(),
                            response_message(&body)
                        )));
                        return;
                    }
                    Err(error) => {
                        if error.is::<VpnRequired>() {
                            let _ = tx.send(WorkerEvent::VpnRequired {
                                task_finished: true,
                                detail: Some(format!(
                                    "退选 {} 时 VPN 验证失效，未继续选 B。请连接后检查 A、B 的实际选课状态。",
                                    current.name()
                                )),
                            });
                            return;
                        }
                        let _ = tx.send(WorkerEvent::Error(format!(
                            "退选 {} 请求异常：{error:#}\n结果未确认，未继续选 B；请核对实际已选课程。",
                            current.name()
                        )));
                        return;
                    }
                }
                thread::sleep(Duration::from_millis(300));
                let select_result = relogin.current_session().and_then(|session| {
                    session.api.select_course(
                        &session.token,
                        &batch.code,
                        &class_type,
                        &fresh_target,
                    )
                });
                match select_result {
                    Ok(body) if selection_succeeded(&body) => {
                        let _ = tx.send(WorkerEvent::RobFinished {
                            successful: vec![fresh_target.name()],
                            pending: Vec::new(),
                            failure: None,
                        });
                        return;
                    }
                    Err(error) if error.is::<VpnRequired>() => {
                        let _ = tx.send(WorkerEvent::VpnRequired {
                            task_finished: true,
                            detail: Some("退 A 后选 B 时 VPN 验证失效，无法确认 B 或自动回选 A。请立即连接 VPN，并在选课系统核对 A、B 状态。".to_owned()),
                        });
                        return;
                    }
                    Err(error) => {
                        let _ = tx.send(WorkerEvent::Error(format!("选 B 请求异常：{error:#}\nB 的结果未确认，已停止，未自动回选 A。请立即核对 A、B 的实际选课状态。")));
                        return;
                    }
                    Ok(body) => {
                        let reason = response_message(&body);
                        let rollback = relogin.current_session().and_then(|session| {
                            session.api.select_course(
                                &session.token,
                                &batch.code,
                                &class_type,
                                &current,
                            )
                        });
                        let rollback_text = match rollback {
                            Ok(body) if selection_succeeded(&body) => "已尝试回选 A".to_owned(),
                            Ok(body) => format!("回选 A 失败：{}", response_message(&body)),
                            Err(error) if error.is::<VpnRequired>() => {
                                let _ = tx.send(WorkerEvent::VpnRequired {
                                    task_finished: true,
                                    detail: Some(format!("B 选课失败：{reason}；回选 A 时 VPN 验证失效。请立即连接 VPN，并在选课系统核对 A、B 状态。")),
                                });
                                return;
                            }
                            Err(error) => format!("回选 A 失败：{error:#}"),
                        };
                        let _ = tx.send(WorkerEvent::Error(format!(
                            "B 选课失败：{reason}；{rollback_text}"
                        )));
                        return;
                    }
                }
            }
            let _ = tx.send(WorkerEvent::RobFinished {
                successful: Vec::new(),
                pending: vec![format!(
                    "{}（未出现空位，未退 {}）",
                    target.name(),
                    current.name()
                )],
                failure: None,
            });
        });
    }

    fn render_course_table(&mut self, ui: &mut egui::Ui) {
        let needle = self.filter.trim().to_lowercase();
        let visible: Vec<usize> = self
            .courses
            .iter()
            .enumerate()
            .filter(|(_, course)| course_visible(course, &self.prefs, &needle))
            .map(|(index, _)| index)
            .collect();

        let table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(48.0))
            .column(Column::initial(120.0).at_least(90.0))
            .column(Column::initial(230.0).at_least(140.0))
            .column(Column::initial(90.0).at_least(70.0))
            .column(Column::initial(190.0).at_least(120.0))
            .column(Column::initial(70.0).at_least(55.0))
            .column(Column::initial(110.0).at_least(80.0))
            .column(Column::initial(300.0).at_least(180.0))
            .column(Column::initial(140.0).at_least(100.0))
            .column(Column::initial(90.0).at_least(70.0))
            .column(Column::initial(90.0).at_least(70.0))
            .column(Column::remainder().at_least(90.0));

        table
            .header(34.0, |mut header| {
                for title in [
                    "选择",
                    "课程号",
                    "课程名",
                    "课序号",
                    "教师",
                    "学分",
                    "课程类别",
                    "上课时间",
                    "上课地点",
                    "课容量",
                    "已选人数",
                    "状态",
                ] {
                    header.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|mut body| {
                for index in visible {
                    let course = &self.courses[index];
                    let values = ["KCH", "KCM", "KXH", "SKJS", "XF", "KCLB", "KRL", "YXRS"]
                        .map(|key| course.text(key));
                    let class_time = course.class_time();
                    let class_location = course.class_location();
                    let status = if course.enrolled() {
                        "已选"
                    } else if course.selectable() {
                        match course.seat_availability() {
                            Some(true) => "未满·可选",
                            Some(false) => "已满",
                            None => "可选·余量未知",
                        }
                    } else if course.has_conflict() {
                        "冲突"
                    } else {
                        "不可选"
                    };
                    body.row(38.0, |mut row| {
                        row.col(|ui| {
                            let mut selected = self.selected_courses.contains(&index);
                            if ui.checkbox(&mut selected, "").changed() {
                                if selected {
                                    self.selected_courses.insert(index);
                                } else {
                                    self.selected_courses.remove(&index);
                                }
                            }
                        });
                        for value in &values[..6] {
                            row.col(|ui| {
                                ui.label(value);
                            });
                        }
                        row.col(|ui| {
                            ui.label(class_time);
                        });
                        row.col(|ui| {
                            ui.label(class_location);
                        });
                        for value in &values[6..] {
                            row.col(|ui| {
                                ui.label(value);
                            });
                        }
                        row.col(|ui| {
                            ui.label(status);
                        });
                    });
                }
            });
    }

    fn render_control_tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (page, title) in [
                (ControlPage::Account, "账号与网络"),
                (ControlPage::Courses, "课程筛选"),
                (ControlPage::Task, "抢课任务"),
                (ControlPage::Advanced, "高级设置"),
            ] {
                ui.selectable_value(&mut self.control_page, page, title);
            }
        });
        ui.separator();
        match self.control_page {
            ControlPage::Account => self.render_account_page(ui),
            ControlPage::Courses => self.render_course_page(ui),
            ControlPage::Task => self.render_task_page(ui),
            ControlPage::Advanced => self.render_advanced_page(ui),
        }
    }

    fn render_account_page(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("account-settings")
            .num_columns(4)
            .spacing([14.0, 10.0])
            .show(ui, |ui| {
                ui.label("学号");
                ui.add_sized(
                    [220.0, 30.0],
                    egui::TextEdit::singleline(&mut self.prefs.username),
                );
                ui.label("密码");
                ui.add_sized(
                    [240.0, 30.0],
                    egui::TextEdit::singleline(&mut self.prefs.password).password(true),
                );
                ui.end_row();
            });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.busy && !self.rob_running,
                    egui::Button::new("登录并抓取课程"),
                )
                .clicked()
            {
                self.login();
            }
            if ui
                .add_enabled(
                    !self.busy && !self.rob_running,
                    egui::Button::new("检测校园网/VPN"),
                )
                .clicked()
            {
                self.check_network();
            }
            if ui
                .add_enabled(
                    !self.busy && !self.rob_running,
                    egui::Button::new("VPN 浏览器授权"),
                )
                .clicked()
            {
                self.authorize_vpn();
            }
            ui.hyperlink_to("打开校园 VPN", VPN_PORTAL);
        });
        if self.vpn_required {
            ui.colored_label(egui::Color32::from_rgb(160, 70, 0), VPN_GUIDANCE);
        }
    }

    fn render_course_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("搜索课程");
            ui.add_sized(
                [320.0, 30.0],
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("课程号 / 课程名 / 教师 / 上课地点"),
            );
            ui.checkbox(&mut self.prefs.only_selectable, "仅显示可选课程");
            ui.checkbox(&mut self.prefs.only_available, "仅显示未满课程")
                .on_hover_text("已选课程仍会保留显示，便于有位换课");
        });
        ui.horizontal(|ui| {
            ui.label(format!("课程 {} 门", self.courses.len()));
            ui.separator();
            ui.label(format!("已勾选 {} 门", self.selected_courses.len()));
            if let Some(batch) = &self.current_batch {
                ui.separator();
                ui.label(format!("当前轮次：{}", batch.name));
            }
        });
    }

    fn render_task_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(
                &mut self.prefs.select_any_available,
                "任意未满课程自动选（无需勾选）",
            )
                .on_hover_text("持续 POST 查询未满课程（SFYM=0），按返回顺序尝试，第一门成功后停止");
            ui.checkbox(&mut self.prefs.auto_relogin, "掉线自动登录")
                .on_hover_text("仅在明确返回登录页时触发；每个任务最多一次。换课已开始退 A 后不会重放不确定请求");
            ui.checkbox(&mut self.prefs.keep_alive, "保活防踢");
        });
        egui::Grid::new("task-settings")
            .num_columns(6)
            .spacing([14.0, 10.0])
            .show(ui, |ui| {
                ui.label("定时开始");
                ui.add_sized(
                    [150.0, 30.0],
                    egui::TextEdit::singleline(&mut self.prefs.scheduled_start)
                        .hint_text("HH:MM:SS / 留空立即"),
                );
                ui.label("轮数");
                ui.add(egui::DragValue::new(&mut self.prefs.click_times).range(1..=100_000));
                ui.label("点击间隔(ms)");
                ui.add(egui::DragValue::new(&mut self.prefs.click_interval_ms).range(0..=60_000));
                ui.end_row();
            });
        let selected_count = self.selected_courses.len();
        if self.prefs.select_any_available {
            ui.colored_label(
                egui::Color32::from_rgb(170, 60, 0),
                "任意课程模式：无需勾选目标，将按查询顺序尝试，并在第一门选课成功后停止",
            );
        } else if selected_count == 0 {
            ui.colored_label(
                egui::Color32::from_rgb(170, 90, 0),
                "尚未勾选目标课程，请先到“课程筛选”页选择至少一门课程",
            );
        } else {
            ui.label(format!("已勾选 {selected_count} 门目标课程"));
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.busy
                        && !self.rob_running
                        && (self.prefs.select_any_available || selected_count > 0),
                    egui::Button::new(if self.prefs.select_any_available {
                        "开始任意课程监控"
                    } else {
                        "开始抢选中课程"
                    }),
                )
                .clicked()
            {
                self.start_rob();
            }
            if ui
                .add_enabled(
                    !self.busy && !self.rob_running,
                    egui::Button::new("有位换课 A → B"),
                )
                .on_hover_text("勾选当前已选 A 和目标 B；确认 B 有余量后才退 A")
                .clicked()
            {
                self.start_swap();
            }
            if ui
                .add_enabled(
                    self.rob_running,
                    egui::Button::new(if self.paused.load(Ordering::Relaxed) {
                        "继续任务"
                    } else {
                        "暂停任务"
                    }),
                )
                .clicked()
            {
                let next = !self.paused.load(Ordering::Relaxed);
                self.paused.store(next, Ordering::Relaxed);
            }
            if ui
                .add_enabled(self.rob_running, egui::Button::new("终止任务"))
                .clicked()
            {
                self.stopped.store(true, Ordering::Relaxed);
            }
        });
    }

    fn render_advanced_page(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("advanced-settings")
            .num_columns(6)
            .spacing([14.0, 10.0])
            .show(ui, |ui| {
                ui.label("列表抓取间隔(ms)");
                ui.add(egui::DragValue::new(&mut self.prefs.page_interval_ms).range(0..=60_000));
                ui.label("403 重试次数");
                ui.add(egui::DragValue::new(&mut self.prefs.list_403_retry).range(0..=50));
                ui.label("403 罚时(ms)");
                ui.add(egui::DragValue::new(&mut self.prefs.penalty_ms).range(0..=60_000));
                ui.end_row();
                ui.label("保活间隔(s)");
                ui.add(egui::DragValue::new(&mut self.prefs.keep_alive_seconds).range(1..=3_600));
                ui.label("说明");
                ui.label("403 罚时和点击间隔支持 0；分页抓取最低 1.5 秒");
                ui.label("");
                ui.label("");
                ui.end_row();
            });
    }

    fn poll_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                WorkerEvent::VpnAuthorized => {
                    self.busy = false;
                    self.vpn_required = false;
                    self.login_session = None;
                    self.batch_dialog = false;
                    self.current_batch = None;
                    self.status =
                        "VPN 授权成功：桌面程序已通过验证码接口检测，请重新登录并抓取课程"
                            .to_owned();
                }
                WorkerEvent::NetworkReady => {
                    self.busy = false;
                    self.vpn_required = false;
                    self.status =
                        "网络检测通过：本程序已能访问选课验证码接口，请点击登录".to_owned();
                }
                WorkerEvent::VpnRequired {
                    task_finished,
                    detail,
                } => {
                    self.busy = false;
                    self.stopped.store(true, Ordering::Relaxed);
                    self.paused.store(false, Ordering::Relaxed);
                    self.vpn_required = true;
                    self.control_page = ControlPage::Account;
                    self.login_session = None;
                    self.batch_dialog = false;
                    if task_finished {
                        self.rob_running = false;
                    }
                    if let Some(detail) = detail {
                        self.result_dialog = Some(detail);
                    }
                    self.status = "需要连接校园网或完成校园 VPN 验证".to_owned();
                }
                WorkerEvent::Status(status) => {
                    if !self.vpn_required {
                        self.status = status;
                    }
                }
                WorkerEvent::LoginReady(session) => {
                    self.busy = false;
                    self.batches = session.batches.clone();
                    self.selected_batch = self
                        .batches
                        .iter()
                        .position(|batch| batch.can_select == "1")
                        .unwrap_or(0);
                    self.login_session = Some(session);
                    self.vpn_required = false;
                    self.batch_dialog = true;
                    self.status = "登录成功，请选择课程轮次".to_owned();
                }
                WorkerEvent::SessionRefreshed(session) => {
                    self.batches = session.batches.clone();
                    self.login_session = Some(session);
                    self.vpn_required = false;
                    self.status = "自动重新登录成功，任务继续运行".to_owned();
                }
                WorkerEvent::Courses {
                    session,
                    batch,
                    teaching_class_type,
                    campus,
                    courses,
                    total,
                } => {
                    self.busy = false;
                    self.status = format!(
                        "完成：轮次 {}，类型 {}，抓取 {}/{}",
                        batch.code,
                        teaching_class_type,
                        courses.len(),
                        total
                    );
                    self.login_session = Some(session);
                    self.current_batch = Some(batch);
                    self.control_page = ControlPage::Courses;
                    self.teaching_class_type = teaching_class_type;
                    self.campus = campus;
                    self.courses = courses;
                    self.total = total;
                    self.selected_courses.clear();
                    let output = app_file("xk-courses.json");
                    let values: Vec<&Value> =
                        self.courses.iter().map(|course| &course.raw).collect();
                    if let Ok(text) = serde_json::to_string_pretty(&values) {
                        let _ = fs::write(output, text);
                    }
                }
                WorkerEvent::RobFinished {
                    successful,
                    pending,
                    failure,
                } => {
                    self.rob_running = false;
                    if let Some(reason) = failure {
                        self.status = "提交异常，任务已停止；请核对实际选课结果".to_owned();
                        self.result_dialog = Some(format!(
                            "{reason}\n\n此前报告成功：{}\n未完成或结果待核对：{}",
                            display_names(&successful),
                            display_names(&pending)
                        ));
                        continue;
                    }
                    if !self.vpn_required {
                        self.status = "抢课任务结束".to_owned();
                    }
                    self.result_dialog = Some(format!(
                        "{}成功：{}\n未成功：{}",
                        if self.vpn_required {
                            "VPN 验证失效，任务已停止；重新连接后请核对选课状态。\n"
                        } else {
                            ""
                        },
                        display_names(&successful),
                        display_names(&pending)
                    ));
                }
                WorkerEvent::Error(error) => {
                    self.busy = false;
                    self.rob_running = false;
                    if !self.vpn_required {
                        self.status = "执行失败".to_owned();
                    }
                    self.result_dialog = Some(error);
                }
            }
        }
    }
}

fn course_visible(course: &Course, prefs: &Prefs, needle: &str) -> bool {
    let selectable = !prefs.only_selectable || course.selectable() || course.enrolled();
    let available = !prefs.only_available || course.has_available_seat() || course.enrolled();
    let searchable = needle.is_empty() || course.searchable_text().contains(needle);
    selectable && available && searchable
}

fn available_candidates(courses: Vec<Course>, successful_ids: &BTreeSet<String>) -> Vec<Course> {
    courses
        .into_iter()
        .filter(|course| {
            course.selectable()
                && course.has_available_seat()
                && !successful_ids.contains(&course.id())
        })
        .collect()
}

fn stop_after_submission_error(name: &str, error: &anyhow::Error, stopped: &AtomicBool) -> String {
    stopped.store(true, Ordering::Relaxed);
    format!("{name}：{error:#}\n任务已停止，未继续重试。请先核对官方已选课程，确认本次提交结果。")
}

fn selection_succeeded(body: &Value) -> bool {
    let data_succeeded = match body.get("data") {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_i64() == Some(1),
        Some(Value::String(value)) => matches!(value.trim(), "1" | "true" | "success"),
        Some(Value::Object(value)) => value
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        _ => false,
    };
    let message_succeeded = body
        .get("msg")
        .and_then(Value::as_str)
        .is_some_and(|message| message.contains("成功") && !message.contains("失败"));
    data_succeeded || message_succeeded
}

fn response_message(body: &Value) -> String {
    body.get("msg")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("未知响应")
        .to_owned()
}

impl eframe::App for XkApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_events();
        let ctx = ui.ctx().clone();
        ctx.request_repaint_after(Duration::from_millis(100));
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, egui::Color32::from_rgb(242, 247, 253));
        ui.vertical(|ui| {
            ui.add_space(6.0);
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(252, 253, 255))
                .stroke(egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgb(177, 197, 222),
                ))
                .corner_radius(egui::CornerRadius::same(12))
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading("DNUI 选课助手");
                        ui.separator();
                        ui.label(if self.rob_running {
                            "任务运行中"
                        } else if self.login_session.is_some() {
                            "已登录"
                        } else {
                            "未登录"
                        });
                        if let Some(batch) = &self.current_batch {
                            ui.separator();
                            ui.label(format!("{} · {}", batch.name, self.teaching_class_type));
                        }
                    });
                    ui.add_space(8.0);
                    self.render_control_tabs(ui);
                });
            ui.add_space(6.0);
            egui::Frame::new()
                .fill(if self.vpn_required {
                    egui::Color32::from_rgb(255, 239, 218)
                } else {
                    egui::Color32::from_rgb(224, 237, 253)
                })
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(if self.rob_running {
                            "运行状态"
                        } else {
                            "状态"
                        });
                        ui.separator();
                        ui.colored_label(egui::Color32::from_rgb(28, 79, 145), &self.status);
                    });
                });
            ui.add_space(4.0);
        });

        self.render_course_table(ui);

        if self.batch_dialog {
            egui::Window::new("选择选课轮次")
                .collapsible(false)
                .resizable(true)
                .show(&ctx, |ui| {
                    for (index, batch) in self.batches.iter().enumerate() {
                        let state = if batch.can_select == "1" {
                            "可选".to_owned()
                        } else {
                            format!(
                                "不可选：{}",
                                batch.no_select_reason.as_deref().unwrap_or("未知原因")
                            )
                        };
                        ui.radio_value(
                            &mut self.selected_batch,
                            index,
                            format!(
                                "[{}] {} | {} | {} ~ {} | {}",
                                batch.group,
                                batch.name,
                                batch.school_term_name,
                                batch.begin_time,
                                batch.end_time,
                                state
                            ),
                        );
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("确定").clicked() {
                            self.choose_batch();
                        }
                        if ui.button("取消").clicked() {
                            self.batch_dialog = false;
                        }
                    });
                });
        }

        if let Some(message) = self.result_dialog.clone() {
            egui::Window::new("提示")
                .collapsible(false)
                .resizable(true)
                .show(&ctx, |ui| {
                    ui.label(message);
                    if ui.button("确定").clicked() {
                        self.result_dialog = None;
                    }
                });
        }
    }
}

fn app_file(name: &str) -> PathBuf {
    let directory = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("DNUI-XK");
    let _ = fs::create_dir_all(&directory);
    directory.join(name)
}

fn configure_chinese_font(ctx: &egui::Context) {
    for path in [r"C:\Windows\Fonts\msyh.ttc", r"C:\Windows\Fonts\simhei.ttf"] {
        if let Ok(bytes) = fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "chinese".to_owned(),
                Arc::new(egui::FontData::from_owned(bytes)),
            );
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts
                    .families
                    .entry(family)
                    .or_default()
                    .insert(0, "chinese".to_owned());
            }
            ctx.set_fonts(fonts);
            break;
        }
    }
}

fn wait_interval(milliseconds: u64, stopped: &AtomicBool) {
    let duration = Duration::from_millis(milliseconds);
    let start = Instant::now();
    while start.elapsed() < duration && !stopped.load(Ordering::Relaxed) {
        thread::sleep((duration - start.elapsed().min(duration)).min(Duration::from_millis(100)));
    }
}

fn wait_until(
    schedule: &str,
    stopped: &AtomicBool,
    tx: &Sender<WorkerEvent>,
) -> Result<(), String> {
    let time = NaiveTime::parse_from_str(schedule, "%H:%M:%S")
        .map_err(|_| "定时时间格式应为 HH:MM:SS".to_owned())?;
    let now = Local::now();
    let mut target = Local
        .from_local_datetime(&now.date_naive().and_time(time))
        .single()
        .ok_or("无法计算定时时间")?;
    if target <= now {
        target += chrono::Duration::days(1);
    }
    while Local::now() < target {
        if stopped.load(Ordering::Relaxed) {
            return Ok(());
        }
        let seconds = (target - Local::now()).num_seconds().max(0);
        let _ = tx.send(WorkerEvent::Status(format!(
            "等待定时开始：还剩 {seconds} 秒"
        )));
        thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}

fn display_names(names: &[String]) -> String {
    if names.is_empty() {
        "无".to_owned()
    } else {
        names.join("、")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        WorkerEvent, available_candidates, course_visible, selection_succeeded,
        stop_after_submission_error, wait_interval,
    };
    use crate::model::{Course, Prefs};
    use crate::network::VpnRequired;
    use serde_json::json;
    use std::{collections::BTreeSet, sync::atomic::AtomicBool, time::Instant};

    #[test]
    fn http_success_code_does_not_hide_business_failure() {
        assert!(!selection_succeeded(&json!({
            "code": 200,
            "data": false,
            "msg": "选课失败，课程已满"
        })));
    }

    #[test]
    fn selection_success_accepts_boolean_data_or_explicit_message() {
        assert!(selection_succeeded(&json!({"code": 200, "data": true})));
        assert!(selection_succeeded(&json!({
            "code": 200,
            "msg": "选课成功"
        })));
    }

    #[test]
    fn vpn_error_survives_context_and_reaches_connection_guide() {
        let error = anyhow::Error::new(VpnRequired).context("验证码请求失败");
        assert!(matches!(
            WorkerEvent::from_error(error),
            WorkerEvent::VpnRequired {
                task_finished: false,
                detail: Some(detail)
            }
            if detail == "验证码请求失败"
        ));
        assert!(matches!(
            WorkerEvent::from_error(anyhow::anyhow!("HTTP 401")),
            WorkerEvent::Error(_)
        ));
    }

    #[test]
    fn stopped_task_does_not_wait_for_retry_interval() {
        let start = Instant::now();
        wait_interval(60_000, &AtomicBool::new(true));
        assert!(start.elapsed().as_secs() < 1);
    }

    #[test]
    fn uncertain_submission_stops_further_rounds() {
        let stopped = AtomicBool::new(false);
        let message =
            stop_after_submission_error("测试课程", &anyhow::anyhow!("HTTP 200 HTML"), &stopped);
        assert!(stopped.load(std::sync::atomic::Ordering::Relaxed));
        assert!(message.contains("未继续重试"));
        assert!(message.contains("确认本次提交结果"));
    }

    #[test]
    fn available_filter_hides_full_courses_but_keeps_enrolled_course() {
        let prefs = Prefs {
            only_available: true,
            ..Prefs::with_defaults()
        };
        let available = Course {
            raw: json!({"SFKT":"1", "KRL":"30", "YXRS":"29"}),
        };
        let full = Course {
            raw: json!({"SFKT":"1", "KRL":"30", "YXRS":"30"}),
        };
        let enrolled = Course {
            raw: json!({"_enrolled":true, "KRL":"30", "YXRS":"30"}),
        };
        assert!(course_visible(&available, &prefs, ""));
        assert!(!course_visible(&full, &prefs, ""));
        assert!(course_visible(&enrolled, &prefs, ""));
    }

    #[test]
    fn any_available_mode_uses_all_eligible_courses_without_targets() {
        let courses = vec![
            Course {
                raw: json!({"JXBID":"A", "SFKT":"1", "SFCT":"0", "KRL":"30", "YXRS":"29"}),
            },
            Course {
                raw: json!({"JXBID":"B", "SFKT":"1", "SFCT":"0", "KRL":"30", "YXRS":"28"}),
            },
            Course {
                raw: json!({"JXBID":"C", "SFKT":"1", "SFCT":"1", "KRL":"30", "YXRS":"20"}),
            },
        ];
        let mut already_successful = BTreeSet::new();
        already_successful.insert("A".to_owned());
        let candidates = available_candidates(courses, &already_successful);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id(), "B");
    }
}
