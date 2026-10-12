//! Which workout is next (v2): pinned weekday first, otherwise rotation through
//! the program's unpinned workouts. Derived from the session log; nothing stored.

use super::*;

const DAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

/// A session of the user's program on the date asked about.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TodaySession {
    pub session_id: String,
    pub workout_id: Option<String>,
    pub status: SessionStatus,
}

/// A workout pinned to today that is an outside session (e.g. "Unit PT").
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ExternalToday {
    pub workout_id: String,
    /// `mon_am` … `sun_pm`.
    pub slot: String,
    pub label: String,
    /// Marked attended (a finished session exists for it today).
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct NextWorkout {
    pub date: String,
    pub program_id: Option<String>,
    pub program_name: Option<String>,
    pub week: Option<u32>,
    pub weeks: Option<u32>,
    /// `None` on a rest day, when everything pinned for today is done, or with no program.
    pub workout: Option<WorkoutPlan>,
    /// Why this workout, or why none.
    pub reason: String,
    pub externals: Vec<ExternalToday>,
    /// The user's sessions on this date for this program, in start order.
    pub todays_sessions: Vec<TodaySession>,
}

fn finished(s: &SessionStatus) -> bool {
    matches!(s, SessionStatus::Completed | SessionStatus::Partial)
}

/// See the module docs. `local_date` maps a session's `started_at` (ms) to the user's local date.
pub(crate) fn next_workout(
    program: Option<(Program, u32)>,
    date: NaiveDate,
    user_id: &str,
    sessions: &[Session],
    local_date: &dyn Fn(i64) -> NaiveDate,
    plan: &dyn Fn(&Program, &ProgramWorkout) -> Result<WorkoutPlan>,
) -> Result<NextWorkout> {
    let mut out = NextWorkout {
        date: date.to_string(),
        program_id: None,
        program_name: None,
        week: None,
        weeks: None,
        workout: None,
        reason: "No program is running on this date.".into(),
        externals: vec![],
        todays_sessions: vec![],
    };
    let Some((program, week)) = program else { return Ok(out) };
    out.program_id = Some(program.id.clone());
    out.program_name = Some(program.name.clone());
    (out.week, out.weeks) = (Some(week), Some(program.weeks));

    let mine = || sessions.iter().filter(|s| s.user_id == user_id && !s.deleted && s.program_id.as_deref() == Some(&program.id));
    let mut today: Vec<&Session> = mine().filter(|s| local_date(s.started_at) == date).collect();
    today.sort_by(|a, b| (a.started_at, &a.id).cmp(&(b.started_at, &b.id)));
    out.todays_sessions = today.iter().map(|s| TodaySession { session_id: s.id.clone(), workout_id: s.workout_id.clone(), status: s.status.clone() }).collect();
    let done_today = |w: &ProgramWorkout| today.iter().any(|s| s.workout_id.as_deref() == Some(&w.id) && finished(&s.status));

    let day_idx = date.weekday().num_days_from_monday() as usize;
    let prefix = &SLOT_KEYS[day_idx * 2][..3];
    let mut pinned: Vec<&ProgramWorkout> = program.workouts.values().filter(|w| w.slot.as_deref().is_some_and(|s| s.starts_with(prefix))).collect();
    pinned.sort_by(|a, b| (&a.slot, a.index).cmp(&(&b.slot, b.index)));

    out.externals = pinned
        .iter()
        .filter_map(|w| Some(ExternalToday { workout_id: w.id.clone(), slot: w.slot.clone()?, label: w.external_label.clone()?, done: done_today(w) }))
        .collect();
    let lifts: Vec<&&ProgramWorkout> = pinned.iter().filter(|w| w.external_label.is_none()).collect();

    let chosen = if let Some(w) = lifts.iter().find(|w| !done_today(w)) {
        let half = if w.slot.as_deref().is_some_and(|s| s.ends_with("pm")) { "PM" } else { "AM" };
        out.reason = format!("Pinned for {} {half}.", DAYS[day_idx]);
        Some(**w)
    } else if !lifts.is_empty() {
        out.reason = "Everything planned for today is done.".into();
        None
    } else {
        // Rotation over the workouts that aren't pinned to a weekday, after the last one finished.
        let mut rotation: Vec<&ProgramWorkout> = program.workouts.values().filter(|w| w.slot.is_none() && w.external_label.is_none()).collect();
        rotation.sort_by_key(|w| w.index);
        let last = mine()
            .filter(|s| finished(&s.status))
            .filter_map(|s| Some((s.started_at, &s.id, rotation.iter().position(|w| Some(&w.id) == s.workout_id.as_ref())?)))
            .max();
        match (rotation.is_empty(), last) {
            (true, _) => {
                out.reason = if out.externals.is_empty() { "Rest day: nothing is pinned for today." } else { "Only an outside session is planned today." }.into();
                None
            }
            (false, None) => {
                out.reason = format!("First workout of the program: {}.", rotation[0].name);
                Some(rotation[0])
            }
            (false, Some((_, _, pos))) => {
                let next = rotation[(pos + 1) % rotation.len()];
                out.reason = format!("Next in rotation after {}.", rotation[pos].name);
                Some(next)
            }
        }
    };
    out.workout = chosen.map(|w| plan(&program, w)).transpose()?;
    Ok(out)
}
