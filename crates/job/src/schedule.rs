use std::collections::HashMap;

use crate::resources::Vector;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occupant {
    pub need: Vector,
    pub devices: Vec<String>,
    pub start: u64,
    pub end: Option<u64>,
    pub queue: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub id: u64,
    pub need: Vector,
    pub devices: Vec<String>,
    pub duration: Option<u64>,
    pub admissible_now: bool,
    pub holds_place: bool,
    pub queue: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lane {
    pub parallel: Option<u64>,
    pub paused: bool,
    pub cap: Vector,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Now,
    At(u64),
    AfterUnknownEnd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueHold {
    Paused,
    Behind(u64),
    Places,
    Share,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Planned {
    pub id: u64,
    pub placement: Placement,
    pub hold: Option<QueueHold>,
}

fn later(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    Some(a?.max(b?))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Member {
    end: Option<u64>,
    need: Vector,
}

fn room_at(lane: Lane, members: &[Member], need: Vector, at: u64) -> (bool, bool) {
    let active: Vec<&Member> = members
        .iter()
        .filter(|m| m.end.is_none_or(|end| end > at))
        .collect();
    let place = lane
        .parallel
        .is_none_or(|parallel| (active.len() as u64) < parallel);
    let share = active
        .iter()
        .fold(need, |sum, m| sum.add(m.need))
        .fits_within(lane.cap);
    (place, share)
}

fn free_place(now: u64, lane: Lane, members: &[Member], need: Vector) -> Option<u64> {
    let mut times: Vec<u64> = members
        .iter()
        .filter_map(|m| m.end)
        .filter(|&end| end > now)
        .collect();
    times.push(now);
    times.sort_unstable();
    times.dedup();
    times.into_iter().find(|&at| {
        let (place, share) = room_at(lane, members, need, at);
        place && share
    })
}

pub fn plan(
    now: u64,
    capacity: Vector,
    lanes: &HashMap<String, Lane>,
    running: &[Occupant],
    queued: &[Candidate],
) -> Vec<Planned> {
    let mut occupants: Vec<Occupant> = running.to_vec();
    let mut members: HashMap<String, Vec<Member>> = HashMap::new();
    for occupant in running {
        if let Some(queue) = &occupant.queue {
            members.entry(queue.clone()).or_default().push(Member {
                end: occupant.end,
                need: occupant.need,
            });
        }
    }
    let mut last: HashMap<String, (u64, Option<u64>)> = HashMap::new();
    let mut placements = Vec::with_capacity(queued.len());
    for job in queued {
        let lane = job
            .queue
            .as_ref()
            .and_then(|q| lanes.get(q).map(|lane| (q, *lane)));
        let gate = lane.map(|(queue, lane)| {
            if lane.paused {
                return (None, Some(QueueHold::Paused));
            }
            let own = members.get(queue).map_or(&[][..], Vec::as_slice);
            let place = free_place(now, lane, own, job.need);
            match last.get(queue) {
                Some(&(before, start)) if start != Some(now) => {
                    (later(start, place), Some(QueueHold::Behind(before)))
                }
                _ if place == Some(now) => (place, None),
                _ if room_at(lane, own, job.need, now).0 => (place, Some(QueueHold::Share)),
                _ => (place, Some(QueueHold::Places)),
            }
        });
        let (placement, hold, start) = match gate {
            Some((start, Some(hold))) => {
                let placement = start.map_or(Placement::AfterUnknownEnd, Placement::At);
                (placement, Some(hold), start)
            }
            _ => {
                let fit = earliest_fit(now, capacity, &occupants, job);
                let placement = match fit {
                    Some(start) if start == now && job.admissible_now => Placement::Now,
                    Some(start) if start > now => Placement::At(start),
                    _ => Placement::AfterUnknownEnd,
                };
                let blocked_by_host = fit == Some(now) && !job.admissible_now;
                let (start, end) = match placement {
                    Placement::Now => (now, job.duration.map(|d| now + d)),
                    Placement::At(start) => (start, job.duration.map(|d| start + d)),
                    Placement::AfterUnknownEnd => (now, None),
                };
                if !blocked_by_host || job.holds_place {
                    occupants.push(Occupant {
                        need: job.need,
                        devices: job.devices.clone(),
                        start,
                        end,
                        queue: job.queue.clone(),
                    });
                }
                let known_start = match placement {
                    Placement::AfterUnknownEnd => None,
                    _ => Some(start),
                };
                (placement, None, known_start)
            }
        };
        if let Some((queue, _)) = lane {
            let end = start
                .zip(job.duration)
                .map(|(start, duration)| start + duration);
            members.entry(queue.clone()).or_default().push(Member {
                end,
                need: job.need,
            });
            last.insert(queue.clone(), (job.id, start));
        }
        placements.push(Planned {
            id: job.id,
            placement,
            hold,
        });
    }
    placements
}

fn earliest_fit(
    now: u64,
    capacity: Vector,
    occupants: &[Occupant],
    job: &Candidate,
) -> Option<u64> {
    let mut starts: Vec<u64> = occupants
        .iter()
        .filter_map(|o| o.end)
        .filter(|&end| end > now)
        .collect();
    starts.push(now);
    starts.sort_unstable();
    starts.dedup();
    starts.into_iter().find(|&start| {
        fits(
            capacity,
            occupants,
            job,
            start,
            job.duration.map(|d| start + d),
        )
    })
}

fn fits(
    capacity: Vector,
    occupants: &[Occupant],
    job: &Candidate,
    start: u64,
    end: Option<u64>,
) -> bool {
    let overlaps = |o: &Occupant| {
        end.is_none_or(|end| o.start < end) && o.end.is_none_or(|o_end| o_end > start)
    };
    let device_taken = occupants
        .iter()
        .filter(|o| overlaps(o))
        .any(|o| o.devices.iter().any(|d| job.devices.contains(d)));
    if device_taken {
        return false;
    }
    let mut points: Vec<u64> = occupants
        .iter()
        .map(|o| o.start)
        .filter(|&s| s > start && end.is_none_or(|end| s < end))
        .collect();
    points.push(start);
    points.into_iter().all(|point| {
        occupants
            .iter()
            .filter(|o| o.start <= point && o.end.is_none_or(|e| e > point))
            .fold(job.need, |sum, o| sum.add(o.need))
            .fits_within(capacity)
    })
}

pub fn extended_end(start: u64, predicted: u64, now: u64) -> u64 {
    const FIRST_EXTENSION_MS: u64 = 60_000;
    const LATER_EXTENSION_BASE_MS: u64 = 15 * 60_000;
    let mut end = start + predicted;
    let mut extension = 1u32;
    while end <= now {
        end += if extension == 1 {
            FIRST_EXTENSION_MS
        } else {
            LATER_EXTENSION_BASE_MS << (extension - 2).min(20)
        };
        extension += 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::MILLI;

    fn cores(n: u64) -> Vector {
        Vector {
            cores_milli: n * MILLI,
            memory: 0,
            pids: 0,
        }
    }

    fn capacity() -> Vector {
        cores(4)
    }

    fn placements(
        now: u64,
        lanes: &HashMap<String, Lane>,
        running: &[Occupant],
        queued: &[Candidate],
    ) -> Vec<(u64, Placement)> {
        plan(now, capacity(), lanes, running, queued)
            .into_iter()
            .map(|p| (p.id, p.placement))
            .collect()
    }

    fn running(n: u64, end: Option<u64>) -> Occupant {
        Occupant {
            need: cores(n),
            devices: vec![],
            start: 0,
            end,
            queue: None,
        }
    }

    fn queued(id: u64, n: u64, duration: Option<u64>) -> Candidate {
        Candidate {
            id,
            need: cores(n),
            devices: vec![],
            duration,
            admissible_now: true,
            holds_place: true,
            queue: None,
        }
    }

    #[test]
    fn a_job_that_fits_starts_now() {
        let plan = placements(
            10,
            &HashMap::new(),
            &[running(2, Some(100))],
            &[queued(1, 2, Some(50))],
        );
        assert_eq!(plan, vec![(1, Placement::Now)]);
    }

    #[test]
    fn two_jobs_that_do_not_fit_together_are_serialised() {
        let plan = placements(
            10,
            &HashMap::new(),
            &[running(3, Some(100))],
            &[queued(1, 3, Some(50))],
        );
        assert_eq!(plan, vec![(1, Placement::At(100))]);
    }

    #[test]
    fn a_short_job_backfills_before_a_waiting_large_one() {
        let plan = placements(
            10,
            &HashMap::new(),
            &[running(2, Some(100))],
            &[queued(1, 4, Some(50)), queued(2, 2, Some(80))],
        );
        assert_eq!(plan, vec![(1, Placement::At(100)), (2, Placement::Now)]);
    }

    #[test]
    fn a_job_that_would_delay_a_waiting_large_one_does_not_backfill() {
        let plan = placements(
            10,
            &HashMap::new(),
            &[running(2, Some(100))],
            &[queued(1, 4, Some(50)), queued(2, 2, Some(200))],
        );
        assert_eq!(plan, vec![(1, Placement::At(100)), (2, Placement::At(150))]);
    }

    #[test]
    fn a_job_of_unknown_length_does_not_backfill_before_a_waiting_large_one() {
        let plan = placements(
            10,
            &HashMap::new(),
            &[running(2, Some(100))],
            &[queued(1, 4, Some(50)), queued(2, 1, None)],
        );
        assert_eq!(plan, vec![(1, Placement::At(100)), (2, Placement::At(150))]);
    }

    #[test]
    fn behind_a_running_job_of_unknown_length_nothing_overtakes_a_waiting_one() {
        let plan = placements(
            10,
            &HashMap::new(),
            &[running(2, None)],
            &[queued(1, 4, Some(50)), queued(2, 2, None)],
        );
        assert_eq!(
            plan,
            vec![
                (1, Placement::AfterUnknownEnd),
                (2, Placement::AfterUnknownEnd)
            ]
        );
    }

    #[test]
    fn a_job_held_back_by_the_host_holds_its_place() {
        let mut first = queued(1, 1, Some(50));
        first.admissible_now = false;
        let plan = placements(10, &HashMap::new(), &[], &[first, queued(2, 4, Some(10))]);
        assert_eq!(
            plan,
            vec![
                (1, Placement::AfterUnknownEnd),
                (2, Placement::AfterUnknownEnd)
            ]
        );
    }

    #[test]
    fn a_job_the_host_blocks_for_reasons_no_job_can_relieve_holds_back_nobody() {
        let mut first = queued(1, 1, Some(50));
        first.admissible_now = false;
        first.holds_place = false;
        let plan = placements(10, &HashMap::new(), &[], &[first, queued(2, 4, Some(10))]);
        assert_eq!(
            plan,
            vec![(1, Placement::AfterUnknownEnd), (2, Placement::Now)]
        );
    }

    #[test]
    fn an_exclusive_device_is_held_by_one_job_at_a_time() {
        let gpu = |mut o: Occupant| {
            o.devices = vec!["gpu0".to_string()];
            o
        };
        let mut job = queued(1, 1, Some(10));
        job.devices = vec!["gpu0".to_string()];
        let plan = placements(10, &HashMap::new(), &[gpu(running(1, Some(100)))], &[job]);
        assert_eq!(plan, vec![(1, Placement::At(100))]);
    }

    #[test]
    fn a_job_that_has_run_past_its_prediction_is_extended_by_a_minute_then_doubling_quarters() {
        assert_eq!(extended_end(0, 1_000, 500), 1_000);
        assert_eq!(extended_end(0, 1_000, 1_000), 61_000);
        assert_eq!(extended_end(0, 1_000, 61_000), 961_000);
        assert_eq!(extended_end(0, 1_000, 961_000), 2_761_000);
    }

    fn unlimited() -> Vector {
        Vector {
            cores_milli: u64::MAX,
            memory: u64::MAX,
            pids: u64::MAX,
        }
    }

    fn lane(parallel: Option<u64>, paused: bool) -> HashMap<String, Lane> {
        HashMap::from([(
            "serial".to_string(),
            Lane {
                parallel,
                paused,
                cap: unlimited(),
            },
        )])
    }

    fn in_serial<T>(mut item: T, set: impl Fn(&mut T)) -> T {
        set(&mut item);
        item
    }

    fn queued_in(id: u64, n: u64, duration: Option<u64>) -> Candidate {
        in_serial(queued(id, n, duration), |c| {
            c.queue = Some("serial".to_string())
        })
    }

    fn running_in(n: u64, end: Option<u64>) -> Occupant {
        in_serial(running(n, end), |o| o.queue = Some("serial".to_string()))
    }

    #[test]
    fn a_queue_of_one_place_runs_its_jobs_one_after_another() {
        let plan = plan(
            10,
            capacity(),
            &lane(Some(1), false),
            &[],
            &[queued_in(1, 1, Some(50)), queued_in(2, 1, Some(50))],
        );
        assert_eq!(plan[0].placement, Placement::Now);
        assert_eq!(plan[1].placement, Placement::At(60));
        assert_eq!(plan[1].hold, Some(QueueHold::Places));
    }

    #[test]
    fn a_queue_of_two_places_runs_two_at_once_and_the_third_after_the_first_ends() {
        let plan = placements(
            10,
            &lane(Some(2), false),
            &[],
            &[
                queued_in(1, 1, Some(50)),
                queued_in(2, 1, Some(80)),
                queued_in(3, 1, Some(10)),
            ],
        );
        assert_eq!(
            plan,
            vec![
                (1, Placement::Now),
                (2, Placement::Now),
                (3, Placement::At(60))
            ]
        );
    }

    #[test]
    fn a_later_job_of_a_queue_never_overtakes_an_earlier_one() {
        let plan = plan(
            10,
            capacity(),
            &lane(Some(2), false),
            &[running(2, Some(100))],
            &[queued_in(1, 4, Some(50)), queued_in(2, 1, Some(10))],
        );
        assert_eq!(plan[0].placement, Placement::At(100));
        assert_eq!(plan[1].placement, Placement::At(100));
        assert_eq!(plan[1].hold, Some(QueueHold::Behind(1)));
    }

    #[test]
    fn a_job_waiting_for_its_queue_holds_no_place_in_the_pool() {
        let plan = placements(
            10,
            &lane(Some(1), false),
            &[running_in(1, None)],
            &[queued_in(1, 1, None), queued(2, 3, None)],
        );
        assert_eq!(
            plan,
            vec![(1, Placement::AfterUnknownEnd), (2, Placement::Now)]
        );
    }

    #[test]
    fn a_paused_queue_starts_nothing_and_holds_back_nobody() {
        let plan = plan(
            10,
            capacity(),
            &lane(Some(1), true),
            &[],
            &[queued_in(1, 1, Some(5)), queued(2, 1, Some(5))],
        );
        assert_eq!(plan[0].placement, Placement::AfterUnknownEnd);
        assert_eq!(plan[0].hold, Some(QueueHold::Paused));
        assert_eq!(plan[1].placement, Placement::Now);
    }

    #[test]
    fn a_queue_without_a_limit_starts_what_the_pool_admits() {
        let plan = placements(
            10,
            &lane(None, false),
            &[],
            &[queued_in(1, 2, Some(5)), queued_in(2, 2, Some(5))],
        );
        assert_eq!(plan, vec![(1, Placement::Now), (2, Placement::Now)]);
    }

    #[test]
    fn a_queue_holding_two_cores_runs_two_jobs_of_two_cores_one_after_another() {
        let mut lanes = lane(None, false);
        lanes.get_mut("serial").unwrap().cap = Vector {
            cores_milli: 2 * MILLI,
            ..unlimited()
        };
        let plan = plan(
            10,
            capacity(),
            &lanes,
            &[],
            &[
                queued_in(1, 2, Some(50)),
                queued_in(2, 2, Some(50)),
                queued(3, 2, Some(50)),
            ],
        );
        assert_eq!(plan[0].placement, Placement::Now);
        assert_eq!(plan[1].placement, Placement::At(60));
        assert_eq!(plan[1].hold, Some(QueueHold::Share));
        assert_eq!(plan[2].placement, Placement::Now);
    }
}
