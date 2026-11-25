//! Population data for charts: a scripted city (the grid player) grown for
//! some days, then one day recorded in detail.
//!
//! ```text
//! cargo run -p citysim_sim --release --example people -- [days] [out_dir] [hierarchy]
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
//!   types through the recorded week (`balance::DAYS_PER_WEEK` days), and
//!   their journals; `week.txt` — their week as a player would read it (what
//!   they did, where, when); `sanity.txt` — every citizen checked through
//!   the week for things that make no sense (asleep away from home, shopping
//!   when the shops are shut, a trip to a building that is gone, working
//!   off shift, a far job while one next door is open, a journal that
//!   contradicts their state).
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
    let days: u32 = args.first().and_then(|a| a.parse().ok()).unwrap_or(5);
    let out = args.get(1).cloned().unwrap_or_else(|| "target/people".into());
    std::fs::create_dir_all(&out).expect("out dir");
    let mut w = World::new(7);
    // `hierarchy` as the third argument: the avenue city.
    let mut p = match args.get(2).map(String::as_str) {
        Some("hierarchy") => GridPlayer::hierarchy(),
        _ => GridPlayer::new(&Zone::ALL),
    };
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
    let mut timeline = String::from("who,hour,activity,purpose,on_road,mode,day\n");
    let mut week = String::new();
    let mut last_state: Vec<String> = vec![String::new(); samples.len()];
    let mut sanity = Sanity::default();
    let mut activity = String::from(
        "hour,sleeping,home,working,shopping,leisure,travelling,v_work,v_home,v_movein,v_moveout,v_shopping,v_goods,v_leisure\n",
    );
    let start = w.time.tick;
    let first_day_end = start + balance::TICKS_PER_DAY as u64;
    let week_end = start + (balance::DAYS_PER_WEEK * balance::TICKS_PER_DAY) as u64;
    let mut trips_first_day = None;
    while w.time.tick < week_end {
        p.act(&mut w);
        w.step();
        let h = w.time.hour();
        if w.time.tick == first_day_end + balance::NIGHT_CHUNKS as u64 + 2 {
            trips_first_day = Some(w.people.trips_by_hour_yesterday);
        }
        if w.time.tick.is_multiple_of(50) {
            sanity.scan(&w);
        }
        for (i, &(c, _)) in samples.iter().enumerate() {
            let line = describe(&w, c);
            if line != last_state[i] {
                let _ = writeln!(week, "{i}|day {} {}|{line}", w.time.day, clock(&w));
                last_state[i] = line;
            }
        }
        if w.time.tick < first_day_end && w.time.tick.is_multiple_of(balance::TICKS_PER_HOUR as u64 / 6) {
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
        if w.time.tick.is_multiple_of(20) {
            for (i, &(c, _)) in samples.iter().enumerate() {
                let Some(v) = inspect::citizen(&w, c) else { continue };
                let (purpose, road, mode) = match v.trip {
                    Some(t) => (
                        format!("{:?}", t.purpose),
                        u8::from(t.vehicle.is_some()),
                        format!("{:?}", t.mode),
                    ),
                    None => (String::new(), 0, String::new()),
                };
                // Day of the recorded week, 1-7.
                let day = w.time.day + 1 - days;
                let _ = writeln!(timeline, "{i},{h:.3},{:?},{purpose},{road},{mode},{day}", v.activity());
            }
        }
    }
    sanity.far_jobs(&w);
    // Trips of the first recorded day (rolled over to yesterday at midnight).
    let mut trips = String::from("hour,to_work,to_home,move_in,move_out,shopping,goods,leisure\n");
    for (h, row) in trips_first_day
        .unwrap_or(w.people.trips_by_hour_yesterday)
        .iter()
        .enumerate()
    {
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
        ("week.txt", &week),
        ("sanity.txt", &sanity.report()),
    ] {
        std::fs::write(format!("{out}/{name}"), s).expect("write");
    }
    w.check_sim_invariants().expect("invariants");
    println!(
        "day {}: {} citizens in {} households; wrote {out}",
        w.time.day, w.stats.population, w.stats.households
    );
    print!("{about}");
    print!("{}", sanity.report());
}

/// `HH:MM` of the sim clock.
fn clock(w: &World) -> String {
    clock_at(w, w.time.tick)
}

/// `HH:MM` at absolute tick `t`.
fn clock_at(w: &World, t: u64) -> String {
    let day = balance::TICKS_PER_DAY as u64;
    let midnight = w.time.tick - w.time.tick_of_day as u64;
    let tod = (t + day - midnight % day) % day;
    let m = tod * 24 * 60 / day;
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// A building as a player would name it, from `c`'s point of view.
fn place_name(w: &World, c: CitizenId, b: Option<BuildingId>) -> String {
    let Some(b) = b else { return "the highway".into() };
    let Some(v) = inspect::citizen(w, c) else {
        return "?".into();
    };
    if b == v.home {
        return "home".into();
    }
    if Some(b) == v.workplace {
        return "work".into();
    }
    match w.buildings.get(b) {
        Some(bb) => format!("{:?} {:?}", bb.kind, b),
        None => format!("a building that is gone ({b:?})"),
    }
}

/// What `c` is doing, in words (changes when they do something else).
fn describe(w: &World, c: CitizenId) -> String {
    let Some(v) = inspect::citizen(w, c) else {
        return "(no longer in the city)".into();
    };
    let plan = inspect::plan(w, c);
    match v.trip {
        Some(t) => {
            let how = match (t.mode, t.vehicle) {
                (TravelMode::Driving, Some(_)) => "driving".to_string(),
                (TravelMode::Driving, None) if t.eta_tick.is_none() => "waiting for the car to pull out".into(),
                (TravelMode::Driving, None) => "driving (off screen)".into(),
                (TravelMode::Passenger, Some(_)) => "in the car with the family".into(),
                (TravelMode::Passenger, None) => "riding along (off screen)".into(),
                (TravelMode::NoCar, _) => "without a car".into(),
                (TravelMode::Walking, _) => "walking".into(),
            };
            format!(
                "{:?} to {} ({how}), expected {}",
                t.purpose,
                place_name(w, c, Some(t.to)),
                clock_at(w, t.expected_tick)
            )
        }
        None => {
            let next = plan.and_then(|p| p.next).map_or(String::new(), |n| {
                let at = n.at_tick.map_or(String::new(), |t| format!(" at {}", clock_at(w, t)));
                format!(
                    "; next: {:?}{}{at}",
                    n.activity,
                    n.purpose.map_or(String::new(), |p| format!(" ({p:?})"))
                )
            });
            format!("{:?} at {}{next}", v.activity(), place_name(w, c, v.inside))
        }
    }
}

/// Things that make no sense, counted over the week with a few examples.
#[derive(Default)]
struct Sanity {
    scans: u32,
    /// Citizens looked at, all scans together.
    looked: u64,
    counts: std::collections::BTreeMap<&'static str, u32>,
    examples: std::collections::BTreeMap<&'static str, Vec<String>>,
}

impl Sanity {
    fn flag(&mut self, kind: &'static str, what: String) {
        *self.counts.entry(kind).or_default() += 1;
        let e = self.examples.entry(kind).or_default();
        if e.len() < 3 {
            e.push(what);
        }
    }

    fn scan(&mut self, w: &World) {
        self.scans += 1;
        self.looked += w.citizens.len() as u64;
        let h = w.time.hour();
        let day = w.time.day;
        for (id, c) in w.citizens.iter() {
            let home = w.households[c.household].home;
            let at = |what: &str| format!("{id:?} day {day} {h:.2}h: {what}");
            match c.location {
                Location::In(b) => {
                    if c.activity == Activity::Sleeping && b != home {
                        self.flag("asleep away from home", at(&format!("{:?}", w.buildings[b].kind)));
                    }
                    if c.activity == Activity::Shopping
                        && !(balance::SHOP_OPEN_H..balance::SHOP_CLOSE_H + balance::SHOP_DURATION_H).contains(&h)
                    {
                        self.flag("shopping while the shops are shut", at("shopping"));
                    }
                    if c.activity == Activity::Leisure && (2.0..7.0).contains(&h) {
                        self.flag("out for leisure in the small hours", at("leisure"));
                    }
                    if c.activity == Activity::Working && c.job.is_none_or(|j| j.workplace != b) {
                        self.flag("working where they have no job", at("working"));
                    }
                    if c.stage == LifeStage::Child && b != home {
                        // (An adult arriving with them may get there a
                        // few ticks later: decisions run in buckets.)
                        let carer = w.households[c.household].citizens.iter().any(|&m| {
                            w.citizens.get(m).is_some_and(|m| {
                                m.drives()
                                    && (m.location == Location::In(b)
                                        || m.location.journey().is_some_and(|j| {
                                            j.to == b && j.arrive_tick.is_some_and(|t| t <= w.time.tick + 10)
                                        }))
                            })
                        });
                        if !carer && c.activity != Activity::Travelling {
                            let adults: Vec<String> = w.households[c.household]
                                .citizens
                                .iter()
                                .filter_map(|&m| w.citizens.get(m))
                                .filter(|m| m.drives())
                                .map(|m| match m.location.journey() {
                                    Some(j) => format!(
                                        "{:?} {:?} from {:?} to {:?} mode {:?} since {}",
                                        m.activity,
                                        j.purpose,
                                        j.from,
                                        j.to,
                                        j.mode,
                                        clock_at(w, j.departed_tick)
                                    ),
                                    None => format!("{:?}@{:?}", m.activity, m.location.building()),
                                })
                                .collect();
                            self.flag(
                                "child alone away from home",
                                at(&format!(
                                    "{:?} at {b:?} until {} adults {adults:?}",
                                    c.activity,
                                    clock_at(w, c.until_tick)
                                )),
                            );
                        }
                    }
                }
                Location::Travelling(j) => {
                    if !w.buildings.contains_key(j.to) {
                        self.flag("travelling to a building that is gone", at(&format!("{:?}", j.purpose)));
                    }
                    if j.purpose == TripPurpose::Shopping && j.to != home && (1.0..6.0).contains(&h) {
                        let since = clock_at(w, j.departed_tick);
                        let on = j
                            .vehicle
                            .and_then(|v| w.vehicles.get(v))
                            .map(|v| (v.speed, v.route.len()));
                        self.flag(
                            "off shopping in the small hours",
                            at(&format!("shopping trip since {since} {:?} car {on:?}", j.mode)),
                        );
                    }
                    let on_road = j.vehicle.is_some_and(|v| w.vehicles.contains_key(v));
                    let hours = w.time.tick.saturating_sub(j.departed_tick) as f32 / balance::TICKS_PER_HOUR as f32;
                    if hours > 4.0 {
                        self.flag(
                            "on a trip for more than 4 hours",
                            at(&format!("{:?} {:?} on road {on_road} {hours:.1}h", j.purpose, j.mode)),
                        );
                    }
                }
            }
            // Journal vs state.
            let last_job = c.journal.iter().rev().find_map(|e| match e.event {
                LifeEvent::GotJob(b) => Some(Some(b)),
                LifeEvent::LostJob => Some(None),
                _ => None,
            });
            if let Some(Some(b)) = last_job
                && c.job.is_none_or(|j| j.workplace != b)
                && c.job.is_some()
            {
                self.flag("journal names another job", at("journal"));
            }
            if c.journal.windows(2).any(|p| p[0].day > p[1].day) {
                self.flag("journal out of order", at("journal"));
            }
        }
        // Cars on the road without their people.
        for (vid, t) in w.traffic.trips.iter() {
            if t.purpose != TripPurpose::Goods && t.party == 0 && t.citizen.is_none_or(|c| !w.citizens.contains_key(c))
            {
                self.flag("car on the road with nobody in it", format!("{vid:?} {:?}", t.purpose));
            }
        }
    }

    /// Workers whose job is far while a vacancy that pays as well is near.
    fn far_jobs(&mut self, w: &World) {
        let vacancies: Vec<([f32; 2], u32)> = w
            .buildings
            .values()
            .filter(|b| b.kind != Zone::Residential && b.occupied < b.capacity)
            .map(|b| {
                (
                    derived::rect_center(b.footprint),
                    people::wage_of(people::job_tier(b), false),
                )
            })
            .collect();
        let (mut far, mut n) = (0, 0);
        for (id, c) in w.citizens.iter() {
            let Some(j) = c.job else { continue };
            if j.part_time {
                continue;
            }
            n += 1;
            let home = derived::rect_center(w.buildings[w.households[c.household].home].footprint);
            let work = derived::rect_center(w.buildings[j.workplace].footprint);
            let d = geom::dist(home, work);
            let near = vacancies
                .iter()
                .filter(|v| v.1 >= j.wage)
                .map(|v| geom::dist(home, v.0))
                .fold(f32::MAX, f32::min);
            if d > 800.0 && near < 250.0 {
                far += 1;
                if far <= 3 {
                    self.examples
                        .entry("far job with an equal one nearby")
                        .or_default()
                        .push(format!("{id:?}: job {d:.0} m away, vacancy {near:.0} m away"));
                }
            }
        }
        self.counts.insert("far job with an equal one nearby", far);
        self.counts.insert("(full-time workers checked)", n);
    }

    fn report(&self) -> String {
        let mut out = format!(
            "sanity: {} scans of every citizen ({} citizen-samples; counts below are samples)\n",
            self.scans, self.looked
        );
        let mut counts = self.counts.clone();
        for k in [
            "asleep away from home",
            "shopping while the shops are shut",
            "out for leisure in the small hours",
            "working where they have no job",
            "child alone away from home",
            "travelling to a building that is gone",
            "off shopping in the small hours",
            "on a trip for more than 4 hours",
            "journal names another job",
            "journal out of order",
            "car on the road with nobody in it",
        ] {
            counts.entry(k).or_default();
        }
        for (k, n) in &counts {
            let _ = writeln!(out, "  {k}: {n}");
            for e in self.examples.get(k).into_iter().flatten() {
                let _ = writeln!(out, "      e.g. {e}");
            }
        }
        out
    }
}
