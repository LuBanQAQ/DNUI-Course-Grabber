use std::{
    collections::BTreeSet,
    fs,
    path::PathBuf,
    sync::{
        Arc,
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
    api::{ApiClient, LoginSession},
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
    },
    Error(String),
}

impl WorkerEvent {
    fn from_error(error: anyhow::Error) -> Self {
        if error.is::<VpnRequired>() {
            Self::VpnRequired {
                task_finished: false,
                detail: None,
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
        if courses.is_empty() {
            self.result_dialog = Some("请先勾选至少一门课程".to_owned());
            return;
        }
        let class_type = self.teaching_class_type.clone();
        let click_times = self.prefs.click_times.max(1);
        let click_interval = self.prefs.click_interval_ms;
        let keep_alive = self.prefs.keep_alive;
        let keep_alive_seconds = self.prefs.keep_alive_seconds.max(1);
        let schedule = self.prefs.scheduled_start.trim().to_owned();
        self.save_prefs();
        self.rob_running = true;
        self.paused.store(false, Ordering::Relaxed);
        self.stopped.store(false, Ordering::Relaxed);
        let paused = self.paused.clone();
        let stopped = self.stopped.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let heartbeat_finished = Arc::new(AtomicBool::new(false));
            let heartbeat = if keep_alive {
                let heartbeat_session = session.clone();
                let heartbeat_stopped = stopped.clone();
                let heartbeat_finished = heartbeat_finished.clone();
                let heartbeat_tx = tx.clone();
                Some(thread::spawn(move || {
                    while !heartbeat_stopped.load(Ordering::Relaxed)
                        && !heartbeat_finished.load(Ordering::Relaxed)
                    {
                        let status = match heartbeat_session.api.heartbeat(&heartbeat_session.token)
                        {
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
            for round in 1..=click_times {
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                while paused.load(Ordering::Relaxed) && !stopped.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(100));
                }
                if pending.is_empty() || stopped.load(Ordering::Relaxed) {
                    break;
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
                    match session.api.select_course(
                        &session.token,
                        &batch.code,
                        &class_type,
                        &course,
                    ) {
                        Ok(body) => {
                            let code = body.get("code").and_then(Value::as_i64).unwrap_or_default();
                            let msg = body.get("msg").and_then(Value::as_str).unwrap_or("");
                            let _ = tx.send(WorkerEvent::Status(format!(
                                "轮次 {round}/{click_times} | {name}: code={code}, {msg}"
                            )));
                            if selection_succeeded(&body) {
                                successful.push(name);
                            } else {
                                next.push(course);
                            }
                        }
                        Err(error) => {
                            if error.is::<VpnRequired>() {
                                stopped.store(true, Ordering::Relaxed);
                                let _ = tx.send(WorkerEvent::from_error(error));
                            } else {
                                let _ = tx.send(WorkerEvent::Status(format!(
                                    "轮次 {round}/{click_times} | {name}: {error}"
                                )));
                            }
                            next.push(course);
                        }
                    }
                    wait_interval(click_interval, &stopped);
                }
                pending = next;
            }
            heartbeat_finished.store(true, Ordering::Relaxed);
            if let Some(heartbeat) = heartbeat {
                let _ = heartbeat.join();
            }
            let pending_names = pending.iter().map(Course::name).collect();
            let _ = tx.send(WorkerEvent::RobFinished {
                successful,
                pending: pending_names,
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
        self.rob_running = true;
        self.stopped.store(false, Ordering::Relaxed);
        thread::spawn(move || {
            for round in 1..=rounds {
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                let fresh = session.api.fetch_courses(
                    &session.token,
                    &batch.code,
                    &class_type,
                    &campus,
                    500,
                    1_500,
                    retries,
                    penalty_ms,
                );
                let (courses, _) = match fresh {
                    Ok(courses) => courses,
                    Err(error) if error.is::<VpnRequired>() => {
                        let _ = tx.send(WorkerEvent::VpnRequired {
                            task_finished: true,
                            detail: Some("VPN 验证失效，换课已停止；尚未提交退课请求。".to_owned()),
                        });
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
                        "换课轮次 {round}/{rounds}：未找到目标 {}",
                        target.name()
                    )));
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
                match session
                    .api
                    .drop_course(&session.token, &batch.code, &class_type, &current)
                {
                    Ok(body) if response_accepted(&body) => {}
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
                            "退选 {} 失败：{error:#}",
                            current.name()
                        )));
                        return;
                    }
                }
                thread::sleep(Duration::from_millis(300));
                match session.api.select_course(
                    &session.token,
                    &batch.code,
                    &class_type,
                    &fresh_target,
                ) {
                    Ok(body) if response_accepted(&body) => {
                        let _ = tx.send(WorkerEvent::RobFinished {
                            successful: vec![fresh_target.name()],
                            pending: Vec::new(),
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
                    result => {
                        let reason = match result {
                            Ok(body) => response_message(&body),
                            Err(error) => format!("{error:#}"),
                        };
                        let rollback = session.api.select_course(
                            &session.token,
                            &batch.code,
                            &class_type,
                            &current,
                        );
                        let rollback_text = match rollback {
                            Ok(body) if response_accepted(&body) => "已尝试回选 A".to_owned(),
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
            });
        });
    }

    fn render_course_table(&mut self, ui: &mut egui::Ui) {
        let needle = self.filter.trim().to_lowercase();
        let visible: Vec<usize> = self
            .courses
            .iter()
            .enumerate()
            .filter(|(_, course)| {
                !self.prefs.only_selectable || course.selectable() || course.enrolled()
            })
            .filter(|(_, course)| needle.is_empty() || course.searchable_text().contains(&needle))
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
                        "可选"
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
                } => {
                    self.rob_running = false;
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

fn response_accepted(body: &Value) -> bool {
    body.get("code").and_then(Value::as_i64) == Some(200) || selection_succeeded(body)
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
            let toolbar_width = ui.available_width();
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(252, 253, 255))
                .stroke(egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgb(177, 197, 222),
                ))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.set_min_width((toolbar_width - 32.0).max(600.0));
                    ui.horizontal(|ui| {
                        ui.heading("选课控制台");
                        ui.separator();
                        ui.label("登录、课程抓取与抢课任务");
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !self.busy && !self.rob_running,
                                egui::Button::new("VPN 浏览器授权"),
                            )
                            .clicked()
                        {
                            self.authorize_vpn();
                        }
                        ui.hyperlink_to("打开校园 VPN（校外先连接）", VPN_PORTAL);
                        if ui
                            .add_enabled(
                                !self.busy && !self.rob_running,
                                egui::Button::new(if self.vpn_required {
                                    "重新检测网络"
                                } else {
                                    "检测校园网/VPN"
                                }),
                            )
                            .clicked()
                        {
                            self.check_network();
                        }
                    });
                    if self.vpn_required {
                        ui.colored_label(egui::Color32::from_rgb(160, 70, 0), VPN_GUIDANCE);
                    }
                    egui::Grid::new("login-grid")
                        .num_columns(6)
                        .spacing([12.0, 9.0])
                        .show(ui, |ui| {
                            ui.label("学号");
                            ui.add_sized(
                                [180.0, 30.0],
                                egui::TextEdit::singleline(&mut self.prefs.username),
                            );
                            ui.label("密码");
                            ui.add_sized(
                                [200.0, 30.0],
                                egui::TextEdit::singleline(&mut self.prefs.password).password(true),
                            );
                            ui.checkbox(&mut self.prefs.only_selectable, "仅显示可选课程");
                            if ui
                                .add_enabled(
                                    !self.busy && !self.rob_running,
                                    egui::Button::new("登录并抓取本轮课程"),
                                )
                                .clicked()
                            {
                                self.login();
                            }
                            ui.end_row();
                            ui.label("抓取间隔(ms)");
                            ui.add(
                                egui::DragValue::new(&mut self.prefs.page_interval_ms)
                                    .range(0..=60_000),
                            );
                            ui.label("403 重试次数");
                            ui.add(
                                egui::DragValue::new(&mut self.prefs.list_403_retry).range(0..=50),
                            );
                            ui.label("搜索");
                            ui.add_sized(
                                [220.0, 30.0],
                                egui::TextEdit::singleline(&mut self.filter)
                                    .hint_text("课程号 / 课程名 / 教师"),
                            );
                            ui.end_row();
                            ui.label("403罚时(ms)");
                            ui.add(
                                egui::DragValue::new(&mut self.prefs.penalty_ms).range(0..=60_000),
                            );
                            ui.label("0 表示不等待");
                            ui.label("");
                            ui.label("");
                            ui.label("");
                            ui.end_row();
                            ui.label("定时(HH:MM:SS)");
                            ui.add_sized(
                                [180.0, 30.0],
                                egui::TextEdit::singleline(&mut self.prefs.scheduled_start)
                                    .hint_text("留空立即开始"),
                            );
                            ui.label("点击间隔(ms)");
                            ui.add(
                                egui::DragValue::new(&mut self.prefs.click_interval_ms)
                                    .range(0..=60_000),
                            );
                            ui.label("点击轮数");
                            ui.add(
                                egui::DragValue::new(&mut self.prefs.click_times)
                                    .range(1..=100_000),
                            );
                            ui.end_row();
                            ui.checkbox(&mut self.prefs.keep_alive, "保活防踢");
                            ui.label("保活间隔(s)");
                            ui.add(
                                egui::DragValue::new(&mut self.prefs.keep_alive_seconds)
                                    .range(1..=3_600),
                            );
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(
                                        !self.busy && !self.rob_running,
                                        egui::Button::new("抢选中课程"),
                                    )
                                    .clicked()
                                {
                                    self.start_rob();
                                }
                                if ui
                                    .add_enabled(
                                        !self.busy && !self.rob_running,
                                        egui::Button::new("有位换课 A→B"),
                                    )
                                    .on_hover_text("勾选当前已选 A 和目标 B；仅在 B 有余量时退 A")
                                    .clicked()
                                {
                                    self.start_swap();
                                }
                            });
                            if ui
                                .add_enabled(
                                    self.rob_running,
                                    egui::Button::new(if self.paused.load(Ordering::Relaxed) {
                                        "继续抢课"
                                    } else {
                                        "暂停抢课"
                                    }),
                                )
                                .clicked()
                            {
                                let next = !self.paused.load(Ordering::Relaxed);
                                self.paused.store(next, Ordering::Relaxed);
                            }
                            if ui
                                .add_enabled(self.rob_running, egui::Button::new("终止抢课"))
                                .clicked()
                            {
                                self.stopped.store(true, Ordering::Relaxed);
                            }
                            ui.end_row();
                        });
                });
            ui.add_space(6.0);
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(224, 237, 253))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(12, 7))
                .show(ui, |ui| {
                    ui.colored_label(egui::Color32::from_rgb(28, 79, 145), &self.status);
                });
            ui.add_space(4.0);
        });

        self.render_course_table(ui);
        if false {
            ui.vertical(|ui| {
                let needle = self.filter.trim().to_lowercase();
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Grid::new("course-table")
                            .striped(true)
                            .min_col_width(75.0)
                            .show(ui, |ui| {
                                for header in [
                                    "选择",
                                    "课程号",
                                    "课程名",
                                    "课序号",
                                    "教师",
                                    "学分",
                                    "课程类别",
                                    "课容量",
                                    "已选人数",
                                    "状态",
                                ] {
                                    ui.strong(header);
                                }
                                ui.end_row();
                                for (index, course) in self.courses.iter().enumerate() {
                                    if self.prefs.only_selectable && !course.selectable() {
                                        continue;
                                    }
                                    if !needle.is_empty()
                                        && !course.searchable_text().contains(&needle)
                                    {
                                        continue;
                                    }
                                    let mut selected = self.selected_courses.contains(&index);
                                    if ui.checkbox(&mut selected, "").changed() {
                                        if selected {
                                            self.selected_courses.insert(index);
                                        } else {
                                            self.selected_courses.remove(&index);
                                        }
                                    }
                                    for key in
                                        ["KCH", "KCM", "KXH", "SKJS", "XF", "KCLB", "KRL", "YXRS"]
                                    {
                                        ui.label(course.text(key));
                                    }
                                    ui.label(if course.selectable() {
                                        "可选"
                                    } else if course.has_conflict() {
                                        "冲突"
                                    } else {
                                        "不可选"
                                    });
                                    ui.end_row();
                                }
                            });
                    });
            });
        }

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
    use super::{WorkerEvent, selection_succeeded, wait_interval};
    use crate::network::VpnRequired;
    use serde_json::json;
    use std::{sync::atomic::AtomicBool, time::Instant};

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
                detail: None
            }
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
}
