//! Payments a member schedules to repeat, worked out from the group's records
//! ([`crate::polls::RecordBody::Schedule`]).
//!
//! A schedule belongs to the member who made it: only their records can stop
//! it, and only their app starts its payments. Each one is an ordinary payment,
//! asked of the schedule's approvers and checked against the group's rules;
//! within a spending limit, to trusted payees, approvers' apps can approve it
//! by themselves. Nothing runs while the author's app is closed: a payment
//! whose date passed more than [`GRACE_SECS`] ago is skipped, not sent late.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::polls::{GroupLog, ProposedPayment, Record, RecordBody};

/// How long after its date a scheduled payment is still started.
pub const GRACE_SECS: u64 = 3 * 24 * 60 * 60;

/// How many missed dates a schedule lists: the latest.
const MAX_MISSED: usize = 12;

/// How often a schedule repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Every {
    /// Every so many days (7 for weekly).
    Days(u32),
    /// Every so many calendar months, on the same day of the month (the last
    /// day, in a month too short for it).
    Months(u32),
}

impl Every {
    pub fn is_valid(&self) -> bool {
        match *self {
            Every::Days(n) => (1..=366).contains(&n),
            Every::Months(n) => (1..=24).contains(&n),
        }
    }

    /// The `n`th date of a schedule starting at `first` (unix seconds).
    pub fn nth(&self, first: u64, n: u64) -> u64 {
        match *self {
            Every::Days(days) => first.saturating_add(n.saturating_mul(u64::from(days) * 86_400)),
            Every::Months(months) => add_months(first, n.saturating_mul(u64::from(months))),
        }
    }

    /// Whether `date` is one of the dates of a schedule starting at `first`.
    pub fn falls_on(&self, first: u64, date: u64) -> bool {
        match *self {
            Every::Days(days) => date >= first && (date - first).is_multiple_of(u64::from(days) * 86_400),
            Every::Months(_) => (0..).map(|n| self.nth(first, n)).take_while(|&d| d <= date && d != u64::MAX).any(|d| d == date),
        }
    }
}

/// `secs` (unix) moved forward `months` calendar months, keeping the time of
/// day, on the same day of the month or the month's last.
pub fn add_months(secs: u64, months: u64) -> u64 {
    let (days, time) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(days as i64);
    let total = m as i64 - 1 + months as i64;
    let (y, m) = (y + total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
    let d = d.min(days_in_month(y, m));
    days_from_civil(y, m, d) as u64 * 86_400 + time
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => 31,
    }
}

/// Days since 1970-01-01 to (year, month, day), and back (Howard Hinnant's
/// algorithms, proleptic Gregorian).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = i64::from(if m > 2 { m - 3 } else { m + 9 });
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// A schedule as the interface shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Schedule {
    pub schedule_id: String,
    /// Hex communication key of the member who made it.
    pub author: String,
    pub payments: Vec<ProposedPayment>,
    pub reason: String,
    pub every: Every,
    pub first_due: u64,
    pub signers: Vec<String>,
    pub stopped: bool,
    /// The payments sent, by date: the transaction each went in.
    pub runs: BTreeMap<u64, String>,
    /// The date due now and not yet sent, within [`GRACE_SECS`] of it: what
    /// the author's app starts.
    pub due_now: Option<u64>,
    /// The next date after now.
    pub next_due: Option<u64>,
    /// Dates that passed unsent beyond the grace period.
    pub missed: Vec<u64>,
}

/// Every schedule in a group, as of `now`.
pub fn schedules(log: &GroupLog, members: &[String], now: u64) -> Vec<Schedule> {
    let current: HashSet<&str> = members.iter().map(String::as_str).collect();
    let mut out = Vec::new();
    for (author, records) in log.records.iter().filter(|(a, _)| current.contains(a.as_str())) {
        let records: Vec<&Record> = records.values().filter(|r| r.check().is_ok()).collect();
        for record in &records {
            let RecordBody::Schedule { schedule_id, payments, reason, every, first_due, signers } = &record.body else {
                continue;
            };
            let stopped = records.iter().any(|r| matches!(&r.body, RecordBody::ScheduleStopped { schedule_id: s } if s == schedule_id));
            let runs: BTreeMap<u64, String> = records
                .iter()
                .filter_map(|r| match &r.body {
                    RecordBody::ScheduleRun { schedule_id: s, due, txid } if s == schedule_id => Some((*due, txid.clone())),
                    _ => None,
                })
                .collect();
            let (mut due_now, mut next_due, mut missed) = (None, None, Vec::new());
            if !stopped {
                // Dates older than the latest few can only be missed or sent.
                let skip = match *every {
                    Every::Days(days) => (now.saturating_sub(GRACE_SECS).saturating_sub(*first_due) / (u64::from(days) * 86_400))
                        .saturating_sub(MAX_MISSED as u64),
                    Every::Months(_) => 0,
                };
                for n in skip.. {
                    let due = every.nth(*first_due, n);
                    if due > now {
                        next_due = Some(due);
                        break;
                    }
                    if runs.contains_key(&due) {
                        continue;
                    }
                    if now - due <= GRACE_SECS {
                        due_now.get_or_insert(due);
                    } else {
                        missed.push(due);
                    }
                }
                missed.drain(..missed.len().saturating_sub(MAX_MISSED));
            }
            out.push(Schedule {
                schedule_id: schedule_id.clone(),
                author: author.clone(),
                payments: payments.clone(),
                reason: reason.clone(),
                every: *every,
                first_due: *first_due,
                signers: signers.clone(),
                stopped,
                runs,
                due_now,
                next_due,
                missed,
            });
        }
    }
    out.sort_by_key(|s| (s.stopped, s.next_due));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400;
    /// 2026-01-31 09:00 UTC.
    const JAN_31: u64 = 1_769_850_000;

    #[test]
    fn months_land_on_the_same_day_or_the_months_last() {
        let date = |secs: u64| {
            let (y, m, d) = civil_from_days((secs / DAY) as i64);
            format!("{y}-{m:02}-{d:02} {}", secs % DAY / 3_600)
        };
        assert_eq!(date(JAN_31), "2026-01-31 9");
        let monthly = Every::Months(1);
        let dates: Vec<String> = (0..4).map(|n| date(monthly.nth(JAN_31, n))).collect();
        assert_eq!(dates, ["2026-01-31 9", "2026-02-28 9", "2026-03-31 9", "2026-04-30 9"]);
        assert_eq!(date(add_months(JAN_31, 13)), "2027-02-28 9");
        assert_eq!(date(add_months(1_706_691_600, 1)), "2024-02-29 9", "a leap year");
        assert_eq!(date(Every::Days(14).nth(JAN_31, 1)), "2026-02-14 9");
        assert!(monthly.falls_on(JAN_31, monthly.nth(JAN_31, 3)));
        assert!(!monthly.falls_on(JAN_31, monthly.nth(JAN_31, 3) + DAY));
        assert!(Every::Days(7).falls_on(JAN_31, JAN_31 + 14 * DAY));
        assert!(!Every::Days(7).falls_on(JAN_31, JAN_31 + 13 * DAY));
        assert!(!Every::Days(7).falls_on(JAN_31, JAN_31 - 7 * DAY), "before it started");
    }

    fn rec(seq: u64, body: RecordBody) -> Record {
        Record { group_id: "g".into(), seq, at: 1, body }
    }
    fn schedule(id: &str) -> RecordBody {
        RecordBody::Schedule {
            schedule_id: id.into(),
            payments: vec![ProposedPayment { recipient: "u1payroll".into(), amount_zatoshis: 100, memo: None }],
            reason: "Salary".into(),
            every: Every::Days(7),
            first_due: JAN_31,
            signers: vec!["aa".repeat(32)],
        }
    }

    #[test]
    fn a_payment_is_due_until_sent_and_skipped_once_its_grace_period_passes() {
        let members = vec!["alice".to_string()];
        let mut log = GroupLog::default();
        log.insert("alice", rec(1, schedule("s")));
        log.insert("bob", rec(1, RecordBody::ScheduleStopped { schedule_id: "s".into() }));
        // A day after the third date: the first two are missed, the third due.
        let now = JAN_31 + 14 * DAY + DAY;
        let s = &schedules(&log, &members, now)[0];
        assert!(!s.stopped, "only its author can stop it");
        assert_eq!(s.due_now, Some(JAN_31 + 14 * DAY));
        assert_eq!(s.missed, vec![JAN_31, JAN_31 + 7 * DAY]);
        assert_eq!(s.next_due, Some(JAN_31 + 21 * DAY));

        log.insert("alice", rec(2, RecordBody::ScheduleRun { schedule_id: "s".into(), due: JAN_31 + 14 * DAY, txid: "ab".repeat(32) }));
        let s = &schedules(&log, &members, now)[0];
        assert_eq!(s.due_now, None, "sent");
        assert_eq!(s.runs.len(), 1);

        log.insert("alice", rec(3, RecordBody::ScheduleStopped { schedule_id: "s".into() }));
        let s = &schedules(&log, &members, now + 30 * DAY)[0];
        assert!(s.stopped);
        assert_eq!((s.due_now, s.next_due), (None, None));
    }

    #[test]
    fn a_schedule_started_long_ago_lists_only_its_latest_missed_dates() {
        let members = vec!["alice".to_string()];
        let mut log = GroupLog::default();
        let RecordBody::Schedule { payments, reason, signers, .. } = schedule("s") else { unreachable!() };
        let body = RecordBody::Schedule { schedule_id: "s".into(), payments, reason, every: Every::Days(1), first_due: 0, signers };
        log.insert("alice", rec(1, body));
        let now = JAN_31 + 3_600;
        let s = &schedules(&log, &members, now)[0];
        assert_eq!(s.missed.len(), MAX_MISSED);
        assert_eq!(*s.missed.last().unwrap(), JAN_31 - 9 * 3_600 - 3 * DAY, "the latest beyond the grace period");
        assert_eq!(s.due_now, Some(JAN_31 - 9 * 3_600 - 2 * DAY), "the earliest still within it");
        assert_eq!(s.next_due, Some(JAN_31 - 9 * 3_600 + DAY));
    }
}
