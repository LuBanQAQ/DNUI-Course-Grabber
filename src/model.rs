use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct WeekRange {
    pub start: u16,
    pub end: u16,
    /// 0 = every week, 1 = odd weeks, 2 = even weeks.
    #[serde(default)]
    pub parity: u8,
}

impl WeekRange {
    pub fn new(start: u16, end: u16, parity: u8) -> Self {
        Self {
            start: start.min(end),
            end: start.max(end),
            parity: if parity <= 2 { parity } else { 0 },
        }
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        self.start <= other.end
            && other.start <= self.end
            && (self.parity == 0 || other.parity == 0 || self.parity == other.parity)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ScheduleEntry {
    pub name: String,
    pub weekday: u8,
    pub start_section: u8,
    pub end_section: u8,
    #[serde(default)]
    pub weeks: Vec<WeekRange>,
}

impl ScheduleEntry {
    pub fn valid(&self) -> bool {
        (1..=7).contains(&self.weekday)
            && self.start_section > 0
            && self.start_section <= self.end_section
            && self.end_section <= 20
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        if self.weekday != other.weekday
            || self.start_section > other.end_section
            || other.start_section > self.end_section
        {
            return false;
        }
        weeks_overlap(&self.weeks, &other.weeks)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ScheduleSnapshot {
    #[serde(default)]
    pub semester: String,
    #[serde(default)]
    pub current_week: String,
    #[serde(default)]
    pub entries: Vec<ScheduleEntry>,
}

impl ScheduleSnapshot {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.entries.len() > 512 {
            anyhow::bail!("课表记录过多");
        }
        if self.entries.iter().any(|entry| !entry.valid()) {
            anyhow::bail!("课表节次或星期数据无效");
        }
        Ok(())
    }
}

fn weeks_overlap(left: &[WeekRange], right: &[WeekRange]) -> bool {
    // A missing week expression means the page did not expose a restriction;
    // treat it as potentially overlapping instead of silently allowing a clash.
    if left.is_empty() || right.is_empty() {
        return true;
    }
    left.iter().any(|a| right.iter().any(|b| a.overlaps(b)))
}

fn parse_week_ranges(value: &str) -> Vec<WeekRange> {
    let parity = if value.contains('单') || value.contains('奇') {
        1
    } else if value.contains('双') || value.contains('偶') {
        2
    } else {
        0
    };
    let Ok(pattern) = regex::Regex::new(r"(\d+)\s*(?:-\s*(\d+))?\s*周") else {
        return Vec::new();
    };
    pattern
        .captures_iter(value)
        .filter_map(|capture| {
            let start = capture.get(1)?.as_str().parse::<u16>().ok()?;
            let end = capture
                .get(2)
                .and_then(|value| value.as_str().parse::<u16>().ok())
                .unwrap_or(start);
            Some(WeekRange::new(start, end, parity))
        })
        .collect()
}

fn parse_section(value: &Value) -> Option<u8> {
    match value {
        Value::String(text) => text.trim().parse().ok(),
        Value::Number(number) => number.as_u64().and_then(|value| u8::try_from(value).ok()),
        _ => None,
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Prefs {
    pub username: String,
    pub password: String,
    pub only_selectable: bool,
    #[serde(default = "default_true")]
    pub avoid_schedule_conflicts: bool,
    pub page_interval_ms: u64,
    pub list_403_retry: usize,
    pub penalty_ms: u64,
    pub click_interval_ms: u64,
    pub click_times: usize,
    pub keep_alive: bool,
    pub keep_alive_seconds: u64,
    pub scheduled_start: String,
}

impl Prefs {
    pub fn with_defaults() -> Self {
        Self {
            only_selectable: true,
            avoid_schedule_conflicts: true,
            page_interval_ms: 1_800,
            list_403_retry: 10,
            penalty_ms: 1_500,
            click_interval_ms: 250,
            click_times: 50,
            keep_alive: true,
            keep_alive_seconds: 25,
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Batch {
    pub code: String,
    pub name: String,
    #[serde(rename = "schoolTermName")]
    pub school_term_name: String,
    #[serde(rename = "beginTime")]
    pub begin_time: String,
    #[serde(rename = "endTime")]
    pub end_time: String,
    #[serde(rename = "canSelect")]
    pub can_select: String,
    #[serde(rename = "noSelectReason")]
    pub no_select_reason: Option<String>,
    #[serde(skip)]
    pub group: String,
}

#[derive(Clone, Debug)]
pub struct Course {
    pub raw: Value,
}

impl Course {
    pub fn expand_api_row(raw: Value) -> Vec<Self> {
        let Some(parent) = raw.as_object() else {
            return vec![Self { raw }];
        };
        let Some(classes) = parent.get("tcList").and_then(Value::as_array) else {
            return vec![Self { raw }];
        };
        if classes.is_empty() {
            return vec![Self { raw }];
        }
        classes
            .iter()
            .filter_map(Value::as_object)
            .map(|class| {
                let mut merged = parent.clone();
                merged.remove("tcList");
                for (key, value) in class {
                    merged.insert(key.clone(), value.clone());
                }
                Self {
                    raw: Value::Object(merged),
                }
            })
            .collect()
    }

    pub fn text(&self, key: &str) -> String {
        match self.raw.get(key) {
            Some(Value::String(value)) => value.clone(),
            Some(Value::Number(value)) => value.to_string(),
            Some(Value::Bool(value)) => value.to_string(),
            _ => String::new(),
        }
    }

    pub fn id(&self) -> String {
        ["JXBID", "clazzId", "teachingClassId"]
            .iter()
            .map(|key| self.text(key))
            .find(|value| !value.is_empty())
            .unwrap_or_default()
    }

    pub fn secret(&self) -> String {
        ["secretVal", "secret", "SECRETVAL"]
            .iter()
            .map(|key| self.text(key))
            .find(|value| !value.is_empty())
            .unwrap_or_default()
    }

    pub fn name(&self) -> String {
        self.text("KCM")
    }

    pub fn class_time(&self) -> String {
        let Some(items) = self.raw.get("SKSJ").and_then(Value::as_array) else {
            return self.text("teachingPlaceHide");
        };

        items
            .iter()
            .map(|item| {
                let text = |key: &str| match item.get(key) {
                    Some(Value::String(value)) => value.clone(),
                    Some(Value::Number(value)) => value.to_string(),
                    _ => String::new(),
                };
                let weekday = match text("SKXQ").as_str() {
                    "1" => "周一",
                    "2" => "周二",
                    "3" => "周三",
                    "4" => "周四",
                    "5" => "周五",
                    "6" => "周六",
                    "7" => "周日",
                    _ => "",
                };
                let weeks = text("SKZCMC");
                let start = text("KSJC");
                let end = text("JSJC");
                let sections = match (start.is_empty(), end.is_empty()) {
                    (false, false) if start != end => format!("第{start}-{end}节"),
                    (false, _) => format!("第{start}节"),
                    _ => String::new(),
                };
                [weeks, weekday.to_owned(), sections]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("；")
    }

    pub fn schedule_entries(&self) -> Vec<ScheduleEntry> {
        let Some(items) = self.raw.get("SKSJ").and_then(Value::as_array) else {
            return Vec::new();
        };
        items
            .iter()
            .filter_map(|item| {
                let weekday = parse_section(item.get("SKXQ")?)?;
                let start_section = parse_section(item.get("KSJC")?)?;
                let end_section = parse_section(item.get("JSJC")?).unwrap_or(start_section);
                if !(1..=7).contains(&weekday) || start_section == 0 || start_section > end_section
                {
                    return None;
                }
                let weeks = item
                    .get("SKZCMC")
                    .and_then(Value::as_str)
                    .map(parse_week_ranges)
                    .unwrap_or_default();
                Some(ScheduleEntry {
                    name: self.name(),
                    weekday,
                    start_section,
                    end_section,
                    weeks,
                })
            })
            .collect()
    }

    pub fn matches_major_period(&self, major_period: u8) -> bool {
        if !(1..=7).contains(&major_period) {
            return true;
        }
        let start_section = major_period.saturating_mul(2).saturating_sub(1);
        let end_section = major_period.saturating_mul(2);
        self.schedule_entries()
            .iter()
            .any(|entry| entry.start_section <= end_section && entry.end_section >= start_section)
    }

    pub fn schedule_conflict(&self, schedule: &[ScheduleEntry]) -> Option<String> {
        self.schedule_entries().iter().find_map(|course_entry| {
            schedule
                .iter()
                .find(|entry| course_entry.overlaps(entry))
                .map(|entry| {
                    let day = ["", "周一", "周二", "周三", "周四", "周五", "周六", "周日"]
                        .get(entry.weekday as usize)
                        .copied()
                        .unwrap_or("未知星期");
                    format!(
                        "{day} 第{}-{}节 {}",
                        entry.start_section, entry.end_section, entry.name
                    )
                })
        })
    }

    pub fn class_location(&self) -> String {
        let mut locations = self
            .raw
            .get("SKSJ")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("YPSJDD"))
            .filter_map(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        locations.dedup();
        if locations.is_empty() {
            self.text("YPSJDD")
        } else {
            locations.join("；")
        }
    }

    pub fn selectable(&self) -> bool {
        self.text("SFKT") == "1" && !self.has_conflict()
    }

    pub fn enrolled(&self) -> bool {
        self.raw.get("_enrolled").and_then(Value::as_bool) == Some(true) || self.text("SFYX") == "1"
    }

    pub fn has_available_seat(&self) -> bool {
        let number = |keys: &[&str]| {
            keys.iter()
                .map(|key| self.text(key))
                .find_map(|value| value.trim().parse::<i64>().ok())
        };
        if let (Some(capacity), Some(selected)) = (
            number(&["KRL", "classCapacity"]),
            number(&["YXRS", "numberOfSelected"]),
        ) {
            return selected < capacity;
        }
        ["QXKCKRL", "YXRSKRL"]
            .iter()
            .map(|key| self.text(key))
            .find_map(|value| {
                let (selected, capacity) = value.split_once('/')?;
                Some(selected.trim().parse::<i64>().ok()? < capacity.trim().parse::<i64>().ok()?)
            })
            .unwrap_or(false)
    }

    pub fn has_conflict(&self) -> bool {
        self.text("SFCT") == "1" || self.text("conflictDesc").contains("冲突")
    }

    pub fn searchable_text(&self) -> String {
        let mut text = ["KCH", "KCM", "KXH", "SKJS", "SKJSZC", "KKDW", "KCLB"]
            .iter()
            .map(|key| self.text(key))
            .collect::<Vec<_>>()
            .join(" ");
        text.push(' ');
        text.push_str(&self.class_time());
        text.push(' ');
        text.push_str(&self.class_location());
        text.to_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::{Course, ScheduleEntry, ScheduleSnapshot, WeekRange};
    use serde_json::json;

    #[test]
    fn selectable_course_requires_no_conflict() {
        let selectable = Course {
            raw: json!({"SFKT":"1", "SFCT":"0", "KCM":"Rust"}),
        };
        let conflict = Course {
            raw: json!({"SFKT":"1", "SFCT":"1", "KCM":"Rust"}),
        };
        assert!(selectable.selectable());
        assert!(!conflict.selectable());
    }

    #[test]
    fn course_id_supports_site_field() {
        let course = Course {
            raw: json!({"JXBID":"class-42"}),
        };
        assert_eq!(course.id(), "class-42");
    }

    #[test]
    fn nested_course_rows_expand_to_teaching_classes() {
        let rows = Course::expand_api_row(json!({
            "KCH":"C01", "KCM":"Course", "tcList":[
                {"JXBID":"J1", "KXH":"001", "SFKT":"1"},
                {"JXBID":"J2", "KXH":"002", "SFKT":"0"}
            ]
        }));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].text("KCM"), "Course");
        assert_eq!(rows[1].id(), "J2");
    }

    #[test]
    fn formats_class_time_and_deduplicates_location() {
        let course = Course {
            raw: json!({"SKSJ":[
                {"SKZCMC":"1-16周", "SKXQ":"5", "KSJC":"5", "JSJC":"6", "YPSJDD":"A7-101"},
                {"SKZCMC":"1-16周", "SKXQ":"3", "KSJC":"5", "JSJC":"6", "YPSJDD":"A7-101"}
            ]}),
        };
        assert_eq!(
            course.class_time(),
            "1-16周 周五 第5-6节；1-16周 周三 第5-6节"
        );
        assert_eq!(course.class_location(), "A7-101");
    }

    #[test]
    fn detects_available_seat_from_site_capacity_fields() {
        let available = Course {
            raw: json!({"KRL":"30", "YXRS":"29"}),
        };
        let full = Course {
            raw: json!({"classCapacity":30, "numberOfSelected":30}),
        };
        assert!(available.has_available_seat());
        assert!(!full.has_available_seat());
    }

    #[test]
    fn enrolled_marker_is_recognized() {
        assert!(
            Course {
                raw: json!({"_enrolled":true})
            }
            .enrolled()
        );
        assert!(
            Course {
                raw: json!({"SFYX":"1"})
            }
            .enrolled()
        );
    }

    #[test]
    fn major_periods_map_to_consecutive_double_sections() {
        let course = Course {
            raw: json!({
                "KCM": "Elective",
                "SKSJ": [{"SKXQ":"2", "KSJC":"3", "JSJC":"4", "SKZCMC":"1-16周"}]
            }),
        };
        assert!(!course.matches_major_period(1));
        assert!(course.matches_major_period(2));
        assert!(!course.matches_major_period(3));
    }

    #[test]
    fn schedule_conflict_checks_weekday_sections_and_weeks() {
        let course = Course {
            raw: json!({
                "KCM": "Elective",
                "SKSJ": [{"SKXQ":"2", "KSJC":"3", "JSJC":"4", "SKZCMC":"3-8周"}]
            }),
        };
        let schedule = vec![ScheduleEntry {
            name: "Existing".to_owned(),
            weekday: 2,
            start_section: 3,
            end_section: 4,
            weeks: vec![WeekRange::new(1, 4, 0)],
        }];
        assert!(course.schedule_conflict(&schedule).is_some());

        let non_overlapping = vec![ScheduleEntry {
            weeks: vec![WeekRange::new(9, 16, 0)],
            ..schedule[0].clone()
        }];
        assert!(course.schedule_conflict(&non_overlapping).is_none());
    }

    #[test]
    fn schedule_snapshot_rejects_invalid_entries() {
        let snapshot = ScheduleSnapshot {
            entries: vec![ScheduleEntry {
                weekday: 8,
                start_section: 1,
                end_section: 2,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(snapshot.validate().is_err());
    }
}

fn default_true() -> bool {
    true
}
