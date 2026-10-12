//! v2 analytics: weekly volume per muscle, e1RM over time, PRs. All derived from
//! the session log at read time; nothing is stored.

use super::progression::{session_bests, strength_sets, ASSUMED_RIR};
use super::*;
use std::collections::BTreeMap;

/// The Monday of `d`'s week.
pub fn week_start(d: NaiveDate) -> NaiveDate {
    d - chrono::Duration::days(i64::from(d.weekday().num_days_from_monday()))
}

/// Sets per muscle in the week (Monday start) containing `week`. Every working set counts
/// 1 for each primary muscle of its exercise and 0.5 for each secondary muscle. Warm-ups
/// don't count; drop and failure sets do. `local_date` maps a session's `started_at` (ms) to
/// the user's local date. All of `MUSCLES` are present (0 when untrained).
pub fn weekly_volume(
    sessions: &[Session],
    exercises: &HashMap<String, Exercise>,
    user_id: &str,
    week: NaiveDate,
    local_date: &dyn Fn(i64) -> NaiveDate,
) -> BTreeMap<String, f64> {
    let start = week_start(week);
    let mut out: BTreeMap<String, f64> = MUSCLES.iter().map(|m| ((*m).to_string(), 0.0)).collect();
    for s in sessions.iter().filter(|s| s.user_id == user_id && !s.deleted && week_start(local_date(s.started_at)) == start) {
        for set in s.sets.values().filter(|x| !matches!(x.kind, Some(SetKind::Warmup) | Some(SetKind::Unknown(_)))) {
            let Some(e) = exercises.get(&set.exercise_id) else { continue };
            for m in &e.primary_muscles {
                *out.entry(m.clone()).or_default() += 1.0;
            }
            for m in &e.secondary_muscles {
                *out.entry(m.clone()).or_default() += 0.5;
            }
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct E1rmPoint {
    pub session_id: String,
    pub started_at: i64,
    pub e1rm: f64,
    pub load_kg: f64,
    pub reps: u32,
    pub rir: u8,
}

/// Best working-set e1RM per finished session, oldest first. A set with no RIR counts as `ASSUMED_RIR`.
pub fn e1rm_series(sessions: &[Session], user_id: &str, exercise_id: &str) -> Vec<E1rmPoint> {
    session_bests(sessions, user_id, exercise_id, ASSUMED_RIR)
        .into_iter()
        .map(|b| E1rmPoint { session_id: b.session_id, started_at: b.started_at, e1rm: b.e1rm, load_kg: b.load, reps: b.reps, rir: b.rir })
        .collect()
}

/// `kind` is `"e1rm"` (the best estimated one-rep max) or `"reps"` (a set no other set
/// beats on both load and reps: the most reps at that load or heavier).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Pr {
    pub exercise_id: String,
    pub kind: String,
    pub load_kg: f64,
    pub reps: u32,
    pub e1rm: f64,
    pub session_id: String,
    pub started_at: i64,
}

/// PRs over a user's finished sessions, for one exercise or all. Ties go to the earliest set.
/// Sorted by exercise ID; the e1RM PR first, then rep PRs from heaviest to lightest.
pub fn prs(sessions: &[Session], user_id: &str, exercise_id: Option<&str>) -> Vec<Pr> {
    let finished: Vec<&Session> = sessions
        .iter()
        .filter(|s| s.user_id == user_id && !s.deleted && matches!(s.status, SessionStatus::Completed | SessionStatus::Partial))
        .collect();
    let mut ids: Vec<&str> = finished.iter().flat_map(|s| s.sets.values().map(|x| x.exercise_id.as_str())).filter(|id| exercise_id.is_none_or(|e| e == *id)).collect();
    ids.sort();
    ids.dedup();

    let mut out = vec![];
    for id in ids {
        // (load, reps, e1rm, started_at, session_id)
        let mut all: Vec<(f64, u32, f64, i64, &str)> = vec![];
        for s in &finished {
            for (load, reps, _, e1rm) in strength_sets(s, id, ASSUMED_RIR) {
                all.push((load, reps, e1rm, s.started_at, s.id.as_str()));
            }
        }
        let pr = |kind: &str, t: &(f64, u32, f64, i64, &str)| Pr {
            exercise_id: id.into(),
            kind: kind.into(),
            load_kg: t.0,
            reps: t.1,
            e1rm: t.2,
            session_id: t.4.into(),
            started_at: t.3,
        };
        all.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.3.cmp(&b.3)).then(a.4.cmp(b.4)));
        if let Some(best) = all.first() {
            out.push(pr("e1rm", best));
        }
        all.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)).then(a.3.cmp(&b.3)).then(a.4.cmp(b.4)));
        let mut most_reps = 0;
        for t in &all {
            if t.1 > most_reps {
                most_reps = t.1;
                out.push(pr("reps", t));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn local(ms: i64) -> NaiveDate {
        chrono::DateTime::from_timestamp_millis(ms).unwrap().date_naive()
    }

    fn exercise(id: &str, primary: &[&str], secondary: &[&str]) -> Exercise {
        Exercise {
            primary_muscles: primary.iter().map(|m| m.to_string()).collect(),
            secondary_muscles: secondary.iter().map(|m| m.to_string()).collect(),
            ..Exercise::new(id.into(), id, MeasurementType::WeightReps)
        }
    }

    /// A finished session on `date` with `(exercise, load, reps, kind)` sets.
    fn session(id: &str, user: &str, date: &str, sets: &[(&str, f64, u32, Option<SetKind>)]) -> Session {
        let mut s = Session::start(id.into(), user);
        s.started_at = d(date).and_hms_opt(12, 0, 0).unwrap().and_utc().timestamp_millis();
        s.status = SessionStatus::Completed;
        for (i, (ex, load, reps, kind)) in sets.iter().enumerate() {
            s.sets.insert(
                format!("{id}-{i}"),
                SetLog { index: i as u32, exercise_id: (*ex).into(), load: Some(*load), reps: Some(*reps), rir: Some(0), kind: kind.clone(), ..Default::default() },
            );
        }
        s
    }

    #[test]
    fn week_starts_on_monday() {
        assert_eq!(week_start(d("2026-10-12")), d("2026-10-12")); // a Monday
        assert_eq!(week_start(d("2026-10-18")), d("2026-10-12")); // Sunday
        assert_eq!(week_start(d("2026-10-19")), d("2026-10-19"));
    }

    #[test]
    fn volume_counts_primary_1_secondary_half_and_skips_warmups() {
        let ex: HashMap<_, _> = [exercise("bench", &["chest"], &["triceps", "shoulders"]), exercise("squat", &["quads", "glutes"], &[])]
            .into_iter()
            .map(|e| (e.id.clone(), e))
            .collect();
        let w = Some(SetKind::Warmup);
        let sessions = [
            session("s1", "alice", "2026-10-12", &[("bench", 60.0, 8, w.clone()), ("bench", 80.0, 8, None), ("bench", 80.0, 7, Some(SetKind::Failure)), ("squat", 100.0, 5, Some(SetKind::Drop))]),
            session("s2", "alice", "2026-10-18", &[("bench", 80.0, 6, Some(SetKind::Working))]), // Sunday, same week
            session("s3", "alice", "2026-10-19", &[("bench", 80.0, 6, None)]),                  // next week
            session("s4", "bob", "2026-10-13", &[("bench", 80.0, 6, None)]),                    // someone else
            session("s5", "alice", "2026-10-11", &[("bench", 80.0, 6, None)]),                  // previous Sunday
        ];
        let v = weekly_volume(&sessions, &ex, "alice", d("2026-10-15"), &local);
        assert_eq!((v["chest"], v["triceps"], v["shoulders"], v["quads"], v["glutes"], v["back"]), (3.0, 1.5, 1.5, 1.0, 1.0, 0.0));
        assert_eq!(v.len(), MUSCLES.len());
        assert_eq!(weekly_volume(&sessions, &ex, "alice", d("2026-10-19"), &local)["chest"], 1.0);
    }

    #[test]
    fn series_and_prs() {
        let sessions = [
            session("s1", "alice", "2026-10-01", &[("bench", 80.0, 5, None), ("bench", 70.0, 9, None)]),
            session("s2", "alice", "2026-10-08", &[("bench", 85.0, 5, None), ("bench", 60.0, 10, None)]),
            session("s3", "alice", "2026-10-15", &[("bench", 80.0, 6, None), ("bench", 90.0, 1, Some(SetKind::Drop)), ("squat", 100.0, 5, None)]),
        ];
        let series = e1rm_series(&sessions, "alice", "bench");
        assert_eq!(series.iter().map(|p| p.session_id.as_str()).collect::<Vec<_>>(), ["s1", "s2", "s3"]);
        // F = reps + RIR(0): s1 best 80×5 → 93.33 (70×9 → 91); s2 85×5 → 99.17; s3 80×6 → 96.
        assert_eq!((series[0].load_kg, series[0].reps), (80.0, 5));
        assert!((series[1].e1rm - 85.0 * (1.0 + 5.0 / 30.0)).abs() < 1e-9);
        assert!((series[2].e1rm - 96.0).abs() < 1e-9, "the drop set is ignored");

        let p = prs(&sessions, "alice", Some("bench"));
        let kinds: Vec<_> = p.iter().map(|x| (x.kind.as_str(), x.load_kg, x.reps, x.session_id.as_str())).collect();
        // e1RM PR: 85×5. Rep PRs, heavy → light, each beating every heavier set on reps:
        // 85×5, 80×6, 70×9, 60×10. 80×5 is dominated by 80×6.
        assert_eq!(
            kinds,
            [("e1rm", 85.0, 5, "s2"), ("reps", 85.0, 5, "s2"), ("reps", 80.0, 6, "s3"), ("reps", 70.0, 9, "s1"), ("reps", 60.0, 10, "s2")]
        );
        assert_eq!(prs(&sessions, "alice", None).iter().filter(|x| x.exercise_id == "squat").count(), 2);
        assert!(prs(&sessions, "bob", None).is_empty());
        assert!(prs(&[], "alice", None).is_empty());
    }

    #[test]
    fn tie_goes_to_the_earliest_set() {
        let sessions = [session("b", "alice", "2026-10-08", &[("bench", 80.0, 5, None)]), session("a", "alice", "2026-10-01", &[("bench", 80.0, 5, None)])];
        let p = prs(&sessions, "alice", Some("bench"));
        assert!(p.iter().all(|x| x.session_id == "a"), "{p:?}");
    }
}
