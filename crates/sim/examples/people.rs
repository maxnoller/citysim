//! Population data for charts: a scripted city (the grid player) grown for
//! some days, then one day recorded in detail.
//!
//! ```text
//! cargo run -p citysim_sim --release --example people -- [days] [out_dir]
//! python3 crates/sim/examples/plot_people.py <out_dir> <shots_prefix>
//! ```
//!
//! Writes to `out_dir` (default `target/people`):
//! * `days.csv` — per day: life stages, age bands, household kinds,
//!   happiness, life events, money.
//! * `activity.csv` — every 10 in-game minutes of the last day: share of
//!   citizens per activity, cars on the road per trip purpose.
//! * `trips.csv` — trips started per hour of the last day, per purpose.
//! * `timeline.csv` / `samples.txt` — three citizens of different household
//!   types through the last day, and their journals.
//! * `households.csv` — every household's money, rent and income at the end.

use std::fmt::Write as _;

use citysim_sim::scenarios::GridPlayer;
use citysim_sim::*;

fn kind_name(k: HouseholdKind) -> &'static str {
    match k {
        HouseholdKind::Single => "single",
        HouseholdKind::Couple => "couple",
        HouseholdKind::Family => "family",
        HouseholdKind::Seniors => "seniors",
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let days: u32 = args.first().and_then(|a| a.parse().ok()).unwrap_or(18);
    let out = args.get(1).cloned().unwrap_or_else(|| "target/people".into());
    std::fs::create_dir_all(&out).expect("out dir");
    let mut w = World::new(7);
    let mut p = GridPlayer::new(&Zone::ALL);
    let mut daily = String::from(
        "day,population,children,teens,adults,seniors,a0,a10,a20,a30,a40,a50,a60,a70,a80,a90,\
         single,couple,family,seniors_hh,happiness,births,deaths,hh_arrived,hh_left,moved_out,couples,retired,avg_money,rent_burden,workforce,employed\n",
    );
    // Grow to the start of the recorded day.
    while w.time.day < days {
        p.act(&mut w);
        w.step();
        if w.time.tick_of_day == balance::NIGHT_CHUNKS + 2 {
            // The night's review is done: yesterday's events are final.
            let s = &w.people.stats;
            let e = &w.people.events_yesterday;
            let _ = write!(daily, "{},{}", w.time.day - 1, w.stats.population);
            for x in s.by_stage {
                let _ = write!(daily, ",{x}");
            }
            for x in s.age_pyramid {
                let _ = write!(daily, ",{x}");
            }
            for x in s.by_kind {
                let _ = write!(daily, ",{x}");
            }
            let _ = writeln!(
                daily,
                ",{:.4},{},{},{},{},{},{},{},{:.1},{:.3},{},{}",
                s.avg_happiness,
                e.births,
                e.deaths,
                e.households_arrived,
                e.households_left,
                e.moved_out,
                e.couples,
                e.retired,
                s.avg_money,
                s.avg_rent_burden,
                w.stats.workforce,
                w.stats.employed
            );
        }
    }
    // Three citizens from different household types: a single worker, a
    // working parent, a senior.
    // (Workers who work on the recorded day; the first such in id order.)
    let day = w.time.day;
    let pick = |w: &World, kind: HouseholdKind, worker: bool| -> Option<CitizenId> {
        w.households.values().filter(|h| h.kind == kind).find_map(|h| {
            h.citizens.iter().copied().find(|&c| {
                let c = &w.citizens[c];
                c.job.is_some_and(|j| j.shift.works_on(day)) == worker
                    && c.drives()
                    && c.location == Location::In(h.home)
            })
        })
    };
    let samples: Vec<(CitizenId, &str)> = [
        (pick(&w, HouseholdKind::Single, true), "single worker"),
        (pick(&w, HouseholdKind::Family, true), "parent in a family"),
        (pick(&w, HouseholdKind::Seniors, false), "retiree"),
    ]
    .into_iter()
    .filter_map(|(c, l)| c.map(|c| (c, l)))
    .collect();
    let mut timeline = String::from("who,hour,activity,purpose,on_road\n");
    let mut activity = String::from(
        "hour,sleeping,home,working,shopping,leisure,travelling,v_work,v_home,v_movein,v_moveout,v_shopping,v_goods,v_leisure\n",
    );
    let start = w.time.tick;
    while w.time.tick < start + balance::TICKS_PER_DAY as u64 {
        p.act(&mut w);
        w.step();
        let h = w.time.hour();
        if w.time.tick.is_multiple_of(balance::TICKS_PER_HOUR as u64 / 6) {
            let a = inspect::activity_shares(&w);
            let mut v = [0u32; TripPurpose::COUNT];
            for t in w.traffic.trips.values() {
                v[t.purpose.index()] += 1;
            }
            let _ = write!(activity, "{h:.3}");
            for x in a {
                let _ = write!(activity, ",{x:.4}");
            }
            for x in v {
                let _ = write!(activity, ",{x}");
            }
            activity.push('\n');
        }
        if w.time.tick_of_day == 12 * balance::TICKS_PER_HOUR {
            // Working at noon vs. who should be: employed citizens whose
            // shift today covers 12:00.
            let day = w.time.day;
            let noon = 12 * balance::TICKS_PER_HOUR;
            let (mut due, mut working, mut due_working) = (0, 0, 0);
            for c in w.citizens.values() {
                let on = c.job.is_some_and(|j| {
                    let s = j.shift;
                    (s.works_on(day) && s.start <= noon && noon < s.start + s.length)
                        || (s.works_on(day.saturating_sub(1)) && s.start + s.length > noon + balance::TICKS_PER_DAY)
                });
                due += u32::from(on);
                working += u32::from(c.activity == Activity::Working);
                due_working += u32::from(on && c.activity == Activity::Working);
            }
            let st = &w.people.stats;
            println!(
                "noon: {} citizens: {} children, {} teens, {} adults, {} seniors; workforce {}, employed {}; \
                 on shift by timetable {} ({:.1}% of all), Working {} ({:.1}%), of those on shift Working {} ({:.1}%)",
                w.stats.population,
                st.by_stage[0],
                st.by_stage[1],
                st.by_stage[2],
                st.by_stage[3],
                w.stats.workforce,
                w.stats.employed,
                due,
                100.0 * due as f32 / w.stats.population as f32,
                working,
                100.0 * working as f32 / w.stats.population as f32,
                due_working,
                100.0 * due_working as f32 / due.max(1) as f32
            );
        }
        if w.time.tick.is_multiple_of(7) {
            for (i, &(c, _)) in samples.iter().enumerate() {
                let Some(v) = inspect::citizen(&w, c) else { continue };
                let (purpose, road) = match v.trip {
                    Some(t) => (format!("{:?}", t.purpose), u8::from(t.vehicle.is_some())),
                    None => (String::new(), 0),
                };
                let _ = writeln!(timeline, "{i},{h:.3},{:?},{purpose},{road}", v.activity());
            }
        }
    }
    // Trips of the recorded day (rolled over to yesterday at midnight).
    let mut trips = String::from("hour,to_work,to_home,move_in,move_out,shopping,goods,leisure\n");
    for (h, row) in w.people.trips_by_hour_yesterday.iter().enumerate() {
        let _ = write!(trips, "{h}");
        for x in row {
            let _ = write!(trips, ",{x}");
        }
        trips.push('\n');
    }
    let mut about = String::new();
    for (i, &(c, label)) in samples.iter().enumerate() {
        let Some(v) = inspect::citizen(&w, c) else {
            let _ = writeln!(about, "{i}|{label}|(left the city)");
            continue;
        };
        let job = v
            .citizen
            .job
            .map(|j| {
                format!(
                    "{:?} job, ${}/day, shift {:.1}h for {:.1}h",
                    j.tier,
                    j.wage,
                    j.shift.start_hour(),
                    j.shift.hours()
                )
            })
            .unwrap_or_else(|| {
                if v.citizen.retired {
                    "retired".into()
                } else {
                    "no job".into()
                }
            });
        let journal: Vec<String> = v
            .citizen
            .journal
            .iter()
            .map(|e| format!("day {}: {:?}", e.day, e.event))
            .collect();
        let _ = writeln!(
            about,
            "{i}|{label}|age {}, {} household of {}, {job}, happiness {:.2}|{}",
            v.age_years(),
            kind_name(v.household.kind),
            v.members.len(),
            v.citizen.happiness.total,
            journal.join("; ")
        );
    }
    let mut hh = String::from("kind,members,money,rent,income\n");
    for (hid, h) in w.households.iter() {
        let income = life::daily_income(&w, hid);
        let _ = writeln!(
            hh,
            "{},{},{},{},{income}",
            kind_name(h.kind),
            h.members,
            h.money,
            h.rent
        );
    }
    for (name, s) in [
        ("days.csv", &daily),
        ("activity.csv", &activity),
        ("trips.csv", &trips),
        ("timeline.csv", &timeline),
        ("samples.txt", &about),
        ("households.csv", &hh),
    ] {
        std::fs::write(format!("{out}/{name}"), s).expect("write");
    }
    w.check_sim_invariants().expect("invariants");
    println!(
        "day {}: {} citizens in {} households; wrote {out}",
        w.time.day, w.stats.population, w.stats.households
    );
    print!("{about}");
}
