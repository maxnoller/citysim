//! People as the interface shows them, as plain data that any layout can
//! display: names and portraits from a person's seed (deterministic, made
//! up here, the simulation knows only numbers), and sheets for a person, a
//! building's people, a car and the whole city. Built from
//! `citysim_sim::inspect`; nothing here knows about panels or colours.

use citysim_sim::citizens::{Activity, CitizenId, HappinessPart, JobTier, LifeEvent, LifeStage, TravelMode};
use citysim_sim::households::HouseholdKind;
use citysim_sim::inspect::{self, AGE_BANDS, TripReason};
use citysim_sim::{BuildingId, Place, TripPurpose, VehicleId, World, Zone, balance};

use super::format;
use super::readout::{Line, Tone};

// ---------------------------------------------------------------- names

const FIRST: [&str; 64] = [
    "Ada", "Bram", "Cleo", "Dario", "Elin", "Femi", "Greta", "Hugo", "Ines", "Jonas", "Kaia", "Lior", "Mara", "Nils",
    "Odile", "Pavel", "Quinn", "Rosa", "Sami", "Tilde", "Umar", "Vera", "Wim", "Xenia", "Yara", "Zeno", "Alba",
    "Benno", "Carys", "Dmitri", "Edda", "Felix", "Gaia", "Henrik", "Ida", "Jasper", "Klara", "Luca", "Mina", "Noor",
    "Otto", "Priya", "Rafael", "Selma", "Teo", "Uma", "Viggo", "Wanda", "Yusuf", "Zora", "Aron", "Bea", "Cyril",
    "Dana", "Emil", "Fleur", "Gideon", "Hana", "Ivo", "Juno", "Kofi", "Lena", "Milo", "Nadia",
];

const LAST: [&str; 48] = [
    "Abbing",
    "Brandt",
    "Castell",
    "Dorsey",
    "Ekdahl",
    "Ferrin",
    "Gallo",
    "Holm",
    "Ibarra",
    "Jansen",
    "Kovac",
    "Lindqvist",
    "Moreau",
    "Novak",
    "Okafor",
    "Pereira",
    "Quist",
    "Rahman",
    "Sato",
    "Tamm",
    "Urquhart",
    "Varga",
    "Weller",
    "Yilmaz",
    "Zeller",
    "Arnaud",
    "Bianchi",
    "Calder",
    "Duarte",
    "Engel",
    "Fournier",
    "Grieg",
    "Haddad",
    "Iversen",
    "Jurado",
    "Keller",
    "Lund",
    "Marsh",
    "Nakamura",
    "Olsen",
    "Petrov",
    "Reyes",
    "Strand",
    "Thorne",
    "Uddin",
    "Vos",
    "Wilde",
    "Ziegler",
];

fn mix(seed: u32, salt: u32) -> u32 {
    let mut x = seed ^ salt.wrapping_mul(0x9E37_79B9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^ (x >> 16)
}

/// First name from a citizen's seed.
pub fn first_name(seed: u32) -> &'static str {
    FIRST[mix(seed, 1) as usize % FIRST.len()]
}

/// Family name from a household's seed (shared by its members).
pub fn family_name(seed: u32) -> &'static str {
    LAST[mix(seed, 2) as usize % LAST.len()]
}

/// A citizen's first name, never the same as an older member's of their
/// household.
pub fn given_name(world: &World, id: CitizenId) -> &'static str {
    let Some(c) = world.citizens.get(id) else {
        return "Someone";
    };
    let taken: Vec<&str> = world
        .households
        .get(c.household)
        .map(|h| {
            h.citizens
                .iter()
                .take_while(|&&m| m != id)
                .filter_map(|&m| world.citizens.get(m).map(|mc| first_name(mc.seed)))
                .collect()
        })
        .unwrap_or_default();
    let k = mix(c.seed, 1) as usize;
    (0..FIRST.len())
        .map(|i| FIRST[(k + i * 7) % FIRST.len()])
        .find(|n| !taken.contains(n))
        .unwrap_or(FIRST[k % FIRST.len()])
}

/// "Mara Holm".
pub fn name(world: &World, id: CitizenId) -> String {
    let Some(c) = world.citizens.get(id) else {
        return "Someone who left".into();
    };
    let family = world.households.get(c.household).map_or(0, |h| h.seed);
    format!("{} {}", given_name(world, id), family_name(family))
}

// ---------------------------------------------------------------- portraits

/// A small face drawn from a seed: colours are sRGB `0..=1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Portrait {
    pub skin: [f32; 3],
    pub hair: [f32; 3],
    pub shirt: [f32; 3],
    /// 0 short, 1 long, 2 bun, 3 none (bald).
    pub hair_style: u8,
    /// Grey hair, small head: from the life stage.
    pub stage: LifeStage,
}

const SKINS: [[f32; 3]; 6] = [
    [0.96, 0.82, 0.70],
    [0.91, 0.74, 0.60],
    [0.80, 0.60, 0.45],
    [0.66, 0.47, 0.34],
    [0.50, 0.35, 0.25],
    [0.36, 0.25, 0.18],
];
const HAIRS: [[f32; 3]; 6] = [
    [0.15, 0.11, 0.08],
    [0.35, 0.22, 0.12],
    [0.62, 0.42, 0.22],
    [0.85, 0.70, 0.42],
    [0.55, 0.20, 0.10],
    [0.08, 0.08, 0.09],
];
const SHIRTS: [[f32; 3]; 8] = [
    [0.30, 0.55, 0.85],
    [0.85, 0.40, 0.35],
    [0.35, 0.70, 0.50],
    [0.90, 0.72, 0.30],
    [0.55, 0.45, 0.80],
    [0.25, 0.30, 0.38],
    [0.90, 0.55, 0.65],
    [0.45, 0.65, 0.70],
];

pub fn portrait(seed: u32, stage: LifeStage) -> Portrait {
    let h = mix(seed, 3);
    let mut hair = HAIRS[(h >> 8) as usize % HAIRS.len()];
    if stage == LifeStage::Senior {
        hair = [0.78, 0.78, 0.80];
    }
    Portrait {
        skin: SKINS[h as usize % SKINS.len()],
        hair,
        shirt: SHIRTS[(h >> 16) as usize % SHIRTS.len()],
        hair_style: ((h >> 24) % 4) as u8,
        stage,
    }
}

// ---------------------------------------------------------------- words

pub fn stage_word(s: LifeStage) -> &'static str {
    match s {
        LifeStage::Child => "child",
        LifeStage::Teen => "teenager",
        LifeStage::Adult => "adult",
        LifeStage::Senior => "senior",
    }
}

pub fn activity_word(a: Activity) -> &'static str {
    match a {
        Activity::Sleeping => "Sleeping",
        Activity::Home => "At home",
        Activity::Working => "Working",
        Activity::Shopping => "Shopping",
        Activity::Leisure => "Out for leisure",
        Activity::Travelling => "On the way",
    }
}

/// Why a trip is made, after a word of going ("Driving out for lunch",
/// "on the way home from work"); `TripReason::text()` is the sentence.
pub fn reason_word(r: TripReason) -> &'static str {
    match r {
        TripReason::ToWork => "to work",
        TripReason::HomeFromWork => "home from work",
        TripReason::MovingIn => "to move in",
        TripReason::MovingOut => "out of town",
        TripReason::Groceries => "to the shops",
        TripReason::ShoppingAfterWork => "to the shops after work",
        TripReason::HomeFromShopping => "home from the shops",
        TripReason::LunchBreak => "to lunch on a break",
        TripReason::BackToWork => "back to work after lunch",
        TripReason::OutForLunch => "out for lunch",
        TripReason::HomeFromLunch => "home from lunch",
        TripReason::NightOut => "for a night out",
        TripReason::HomeFromNightOut => "home from a night out",
        TripReason::Delivery => "with goods",
    }
}

/// The reason of the trip a citizen is on, if any.
fn trip_reason(world: &World, id: CitizenId) -> Option<TripReason> {
    inspect::citizen(world, id).and_then(|v| v.trip).map(|t| t.reason)
}

pub fn purpose_title(p: TripPurpose) -> &'static str {
    match p {
        TripPurpose::ToWork => "To work",
        TripPurpose::ToHome => "Home",
        TripPurpose::MoveIn => "Moving in",
        TripPurpose::MoveOut => "Moving out",
        TripPurpose::Shopping => "Shopping",
        TripPurpose::Goods => "Goods",
        TripPurpose::Leisure => "Leisure",
    }
}

pub fn kind_word(k: HouseholdKind) -> &'static str {
    match k {
        HouseholdKind::Single => "Single",
        HouseholdKind::Couple => "Couple",
        HouseholdKind::Family => "Family",
        HouseholdKind::Seniors => "Seniors",
    }
}

pub fn tier_word(t: JobTier) -> &'static str {
    match t {
        JobTier::Basic => "basic",
        JobTier::Skilled => "skilled",
        JobTier::Professional => "professional",
    }
}

pub fn part_title(p: HappinessPart) -> &'static str {
    match p {
        HappinessPart::Employment => "Work",
        HappinessPart::Commute => "Commute",
        HappinessPart::Affordability => "Money",
        HappinessPart::Access => "Shops & leisure",
        HappinessPart::Taxes => "Taxes",
        HappinessPart::Finances => "City services",
        HappinessPart::Housing => "Home",
    }
}

pub fn mood_word(h: f32) -> &'static str {
    if h >= 0.85 {
        "Very happy"
    } else if h >= 0.7 {
        "Happy"
    } else if h >= balance::UNHAPPY_BELOW {
        "Content"
    } else if h >= 0.45 {
        "Unhappy"
    } else {
        "Miserable"
    }
}

/// "07:30" for a tick of the day.
pub fn clock(tick_of_day: u64) -> String {
    let minutes = tick_of_day * 24 * 60 / balance::TICKS_PER_DAY as u64;
    format!("{:02}:{:02}", (minutes / 60) % 24, minutes % 60)
}

/// "2 h 10 min" for a span of ticks (calendar time).
pub fn span(ticks: u64) -> String {
    let day = balance::TICKS_PER_DAY as u64;
    let minutes = (ticks * 24 * 60 + day / 2) / day;
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// "Shop on Lindqvist Street"-like name of a building: its kind and a
/// family name from its seed.
pub fn building_name(world: &World, b: BuildingId) -> String {
    let Some(bd) = world.buildings.get(b) else {
        return "A building that is gone".into();
    };
    let who = family_name(bd.seed);
    match bd.kind {
        Zone::Residential if bd.capacity > 6 => format!("{who} House flats"),
        Zone::Residential => format!("The {who} home"),
        Zone::Commercial if bd.level >= 3 => format!("{who} Offices"),
        Zone::Commercial => format!("{who}'s shop"),
        Zone::Industrial => format!("{who} Works"),
    }
}

// ---------------------------------------------------------------- sheets

/// Where a link leads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Person(CitizenId),
    Place(BuildingId),
    Vehicle(VehicleId),
}

/// A clickable line: a label, a value and where it leads.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub label: String,
    pub value: String,
    pub target: Target,
}

impl Link {
    pub fn new(label: impl Into<String>, value: impl Into<String>, target: Target) -> Link {
        Link {
            label: label.into(),
            value: value.into(),
            target,
        }
    }
}

/// One line of a sheet: plain or a link.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Line(Line),
    Link(Link),
    /// A labelled share `0..=1` (bars).
    Bar {
        label: String,
        value: String,
        share: f32,
    },
    /// A line that moves the camera to where the selected trip goes.
    Jump {
        label: String,
        value: String,
    },
}

/// A section the player can fold away (its title stays as a one-line
/// summary). The panel remembers each one's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FoldKey {
    Life,
    Household,
    Happiness,
    Journal,
    Residents,
    Workers,
    Aboard,
}

impl FoldKey {
    /// Open until the player folds it.
    pub fn open_by_default(self) -> bool {
        matches!(
            self,
            FoldKey::Life | FoldKey::Residents | FoldKey::Workers | FoldKey::Aboard
        )
    }
}

/// A titled group of items.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Section {
    pub title: String,
    pub items: Vec<Item>,
    /// Foldable (and remembered under this key).
    pub fold: Option<FoldKey>,
}

impl Section {
    pub fn new(title: impl Into<String>) -> Section {
        Section {
            title: title.into(),
            items: Vec::new(),
            fold: None,
        }
    }
    pub fn folding(title: impl Into<String>, key: FoldKey) -> Section {
        Section {
            fold: Some(key),
            ..Section::new(title)
        }
    }
    pub fn line(&mut self, label: impl Into<String>, value: impl Into<String>, tone: Tone) {
        self.items.push(Item::Line(Line::new(label, value, tone)));
    }
    pub fn link(&mut self, label: impl Into<String>, value: impl Into<String>, target: Target) {
        self.items.push(Item::Link(Link::new(label, value, target)));
    }
    pub fn jump(&mut self, label: impl Into<String>, value: impl Into<String>) {
        self.items.push(Item::Jump {
            label: label.into(),
            value: value.into(),
        });
    }
    pub fn bar(&mut self, label: impl Into<String>, value: impl Into<String>, share: f32) {
        self.items.push(Item::Bar {
            label: label.into(),
            value: value.into(),
            share: share.clamp(0.0, 1.0),
        });
    }
}

/// What a sheet is about (the panel picks its icon and actions by it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SheetKind {
    #[default]
    Person,
    Home,
    Workplace,
    Vehicle,
    Road,
    Junction,
    City,
}

/// A trip in progress: what, how long to go, how far along, from and to.
#[derive(Debug, Clone, PartialEq)]
pub struct TripBar {
    pub title: String,
    /// Expected time still to go ("0:40", hours:minutes of game time), or
    /// how late they are ("12 min late").
    pub left: String,
    /// Past the expected arrival and still on the way.
    pub late: bool,
    pub progress: f32,
    pub from: String,
    pub to: String,
}

/// What a people view shows: a heading and sections.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sheet {
    pub kind: SheetKind,
    pub title: String,
    /// The bold line beside the portrait ("Skilled job · Holm Offices").
    pub headline: String,
    pub subtitle: String,
    /// A trip in progress (person, car).
    pub trip: Option<TripBar>,
    /// The part of their happiness that is worst, when it hurts.
    pub mood_reason: Option<String>,
    /// Where the action buttons lead (home, work, the driver).
    pub actions: Vec<Link>,
    /// The face of the person (person sheets).
    pub portrait: Option<Portrait>,
    /// Mood `0..=1` (person sheets).
    pub mood: Option<f32>,
    pub sections: Vec<Section>,
    /// Ground point the camera follows (sim metres).
    pub at: Option<[f32; 2]>,
    /// The car the person is in (to highlight it).
    pub vehicle: Option<VehicleId>,
    /// The building they are in.
    pub inside: Option<BuildingId>,
}

/// Where to look for a person on the map now: their car (at its drawn
/// lane pose), the building they are in, or, on a trip the map does not
/// draw (on foot, or a car beyond the cars shown), the place they are
/// going: there is nothing to see on the way.
pub fn person_position(world: &World, id: CitizenId) -> Option<[f32; 2]> {
    if let Some((_, w)) = citysim_sim::walk::walker_of(world, id) {
        return Some(w.pos);
    }
    let v = inspect::citizen(world, id)?;
    if let Some(t) = v.trip {
        return match t.vehicle.and_then(|c| world.vehicles.get(c)) {
            Some(car) => Some(citysim_sim::drive::vehicle_pose(world, car).0),
            None => world.buildings.get(t.to).map(|b| b.center()),
        };
    }
    inspect::citizen_pos(world, id)
}

/// A trip the map does not draw: on foot, a lift, or a car beyond the cars
/// shown.
pub fn trip_unseen(world: &World, t: &inspect::TripView) -> bool {
    t.vehicle.is_none_or(|c| !world.vehicles.contains_key(c))
        && !(t.mode == TravelMode::Walking && t.eta_tick.is_none())
}

/// Why a trip is not on the map, in words.
fn unseen_words(t: &inspect::TripView) -> &'static str {
    match t.mode {
        TravelMode::Walking => {
            "On foot where the map shows nobody (too many walking at once): the marker shows where they should be \
             by now"
        }
        TravelMode::NoCar => "Without a car they go by lift or bus: the marker shows where they should be by now",
        _ => {
            "Off screen: the map shows up to 125 cars per 1,000 residents, the rest drive in the same traffic. The \
             marker shows where they should be by now"
        }
    }
}

/// Clock time of an absolute tick (the game's tick 0 is not midnight: a
/// new game starts in the morning).
pub fn tick_clock(world: &World, t: u64) -> String {
    let day = balance::TICKS_PER_DAY as i64;
    let tod = (world.time.tick_of_day as i64 + t as i64 - world.time.tick as i64).rem_euclid(day);
    clock(tod as u64)
}

/// What a person is doing, in words, and what comes next.
fn doing(world: &World, id: CitizenId) -> (String, String) {
    let Some(p) = inspect::plan(world, id) else {
        return (String::new(), String::new());
    };
    let now = world.time.tick;
    // Leisure at midday is a lunch: on the way there, or there now (then
    // back to work or home from lunch).
    let lunch_trip = p
        .trip
        .is_some_and(|t| matches!(t.reason, TripReason::LunchBreak | TripReason::OutForLunch));
    let at_lunch = p.trip.is_none()
        && p.activity == Activity::Leisure
        && p.next
            .is_some_and(|n| n.activity == Activity::Working || n.reason == Some(TripReason::HomeFromLunch));
    let activity = |a: Activity, lunch: bool| {
        if lunch && a == Activity::Leisure {
            "At lunch"
        } else {
            activity_word(a)
        }
    };
    let doing = match p.trip {
        Some(t) => {
            let how = match t.mode {
                TravelMode::Driving => "Driving",
                TravelMode::Passenger => "Riding",
                TravelMode::NoCar => "Getting a lift",
                TravelMode::Walking => "Walking",
            };
            format!(
                "{how} {} · {} so far",
                reason_word(t.reason),
                span(now.saturating_sub(t.departed_tick))
            )
        }
        None => match p.until_tick {
            Some(u) if u > now => format!("{} until {}", activity(p.activity, at_lunch), tick_clock(world, u)),
            _ => activity(p.activity, at_lunch).to_string(),
        },
    };
    let next = match p.next {
        Some(n) => {
            let when = n
                .at_tick
                .map(|t| format!(" at {}", tick_clock(world, t)))
                .unwrap_or_default();
            let place = n
                .place
                .filter(|_| n.activity != Activity::Home)
                .map(|b| format!(" · {}", building_name(world, b)))
                .unwrap_or_default();
            match (n.activity, n.purpose, p.trip.is_some()) {
                (_, _, true) => match n.at_tick {
                    Some(t) => format!(
                        "Expected about {}, then {}",
                        tick_clock(world, t),
                        activity(n.activity, lunch_trip).to_lowercase()
                    ),
                    None => format!("Then {}", activity(n.activity, lunch_trip).to_lowercase()),
                },
                (Activity::Travelling, Some(TripPurpose::ToWork), _) => format!("Leaves for work{when}{place}"),
                (Activity::Travelling, Some(TripPurpose::ToHome), _) => match n.reason {
                    Some(
                        r @ (TripReason::HomeFromLunch | TripReason::HomeFromNightOut | TripReason::HomeFromShopping),
                    ) => {
                        format!("Heads {}{when}", reason_word(r))
                    }
                    _ => format!("Heads home{when}"),
                },
                (Activity::Travelling, Some(_), _) if let Some(r) = n.reason => {
                    format!("Sets off {}{when}", reason_word(r))
                }
                (Activity::Travelling, Some(_), _) => format!("Sets off{when}"),
                (Activity::Sleeping, _, _) => format!("Bed{when}"),
                // "Sleeping until 07:06" already says when they get up.
                (Activity::Home, _, _) if p.activity == Activity::Sleeping => String::new(),
                (a, _, _) => format!("Then {}{when}{place}", activity_word(a).to_lowercase()),
            }
        }
        None => String::new(),
    };
    (doing, next)
}

fn journal_words(world: &World, e: LifeEvent) -> String {
    match e {
        LifeEvent::Born => "Was born in the city".into(),
        LifeEvent::MovedIn => "Moved to the city".into(),
        LifeEvent::GotJob(b) => format!("Started work at {}", building_name(world, b)),
        LifeEvent::LostJob => "Lost their job".into(),
        LifeEvent::ChildBorn(c) => format!("{} was born", name(world, c)),
        LifeEvent::BecameTeen => "Became a teenager".into(),
        LifeEvent::BecameAdult => "Grew up".into(),
        LifeEvent::MovedOut => "Moved into a home of their own".into(),
        LifeEvent::Partner(c) => format!("Moved in with {}", name(world, c)),
        LifeEvent::MovedHome => "Moved to a cheaper home".into(),
        LifeEvent::Retired => "Retired".into(),
        LifeEvent::Bereaved(c) => format!("Lost {}", name(world, c)),
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// "0:40": hours and minutes of game time.
pub fn hours_minutes(ticks: u64) -> String {
    let day = balance::TICKS_PER_DAY as u64;
    let minutes = (ticks * 24 * 60 + day / 2) / day;
    format!("{}:{:02}", minutes / 60, minutes % 60)
}

/// The trip bar of a journey.
fn trip_bar(world: &World, t: &inspect::TripView) -> TripBar {
    let now = world.time.tick;
    let how = match t.mode {
        TravelMode::Driving => "Driving",
        TravelMode::Passenger => "Riding",
        TravelMode::NoCar => "Getting a lift",
        TravelMode::Walking => "Walking",
    };
    let done = now.saturating_sub(t.departed_tick);
    let late = now > t.expected_tick;
    let (left, progress) = if late {
        // Past the expected arrival: the bar shows the road still ahead
        // (at the car's way of going), never a full bar.
        let ahead = t
            .vehicle
            .and_then(|v| world.vehicles.get(v))
            .map_or(done / 10, |car| road_ahead_ticks(world, car));
        (
            format!("{} late", span(now - t.expected_tick)),
            (done as f32 / (done + ahead).max(1) as f32).clamp(0.0, 0.97),
        )
    } else {
        let total = t.expected_tick.saturating_sub(t.departed_tick).max(1);
        (
            hours_minutes(t.expected_tick - now),
            (done as f32 / total as f32).clamp(0.0, 1.0),
        )
    };
    TripBar {
        title: format!("{how} {}", reason_word(t.reason)),
        left,
        late,
        progress,
        from: t.from.map_or("The highway".into(), |b| building_name(world, b)),
        to: building_name(world, t.to),
    }
}

/// Ticks a car still needs for the rest of its route, at the speed cars
/// manage in town right now (never below a crawl).
fn road_ahead_ticks(world: &World, car: &citysim_sim::Vehicle) -> u64 {
    let len = |s| world.roads.segments.get(s).map_or(0.0, |seg| seg.length);
    let here = car.segment.map_or(0.0, |s| (len(s) - car.progress).max(0.0));
    // The last segment is driven part of the way: count half of it.
    let n = car.route.len();
    let rest: f32 = car
        .route
        .iter()
        .enumerate()
        .map(|(i, &s)| if i + 1 == n { len(s) * 0.5 } else { len(s) })
        .sum();
    let speed = world.stats.avg_speed.max(3.0);
    ((here + rest) / speed * citysim_sim::TICKS_PER_SECOND as f32) as u64
}

/// The selected person's trip, as the map draws it: where they are going
/// and why, how far and how long it still is, and a jump to the place.
/// Before a trip: the next one they will make, as planned.
fn route_section(
    world: &World,
    id: CitizenId,
    trip: Option<&inspect::TripView>,
    bar: Option<&TripBar>,
    routes: &mut super::route::TripCache,
) -> Option<Section> {
    let r = routes.person(world, id)?;
    let dest = r.destination?;
    let to = dest
        .building
        .map_or("The highway".to_string(), |b| building_name(world, b));
    let km = (!r.points.is_empty()).then(|| super::readout::metres(super::route::path_length(&r.points)));
    let mut sec;
    match (trip, r.state) {
        (Some(t), _) => {
            sec = Section::new("Route");
            sec.jump("Going to", to);
            sec.line("Why", t.reason.text(), Tone::Normal);
            let time = bar.filter(|b| !b.late).map(|b| format!("about {}", b.left));
            let left = match (km, time) {
                (Some(k), Some(t)) => format!("{k} · {t}"),
                (Some(k), None) => k,
                (None, Some(t)) => t,
                (None, None) => String::new(),
            };
            if !left.is_empty() {
                sec.line("Still to go", left, Tone::Normal);
            }
            if r.state == crate::shared::RouteState::Unseen && r.at.is_none() {
                sec.line(
                    "",
                    "Dashed on the map: the way by road (not drawn under way)",
                    Tone::Muted,
                );
            }
        }
        (None, _) => {
            let p = inspect::plan(world, id)?;
            let next = p.next?;
            sec = Section::new("Next trip");
            sec.jump("Going to", to);
            if let Some(r) = next.reason {
                sec.line("Why", r.text(), Tone::Normal);
            } else if let Some(pp) = next.purpose {
                sec.line("Why", purpose_title(pp), Tone::Normal);
            }
            if let Some(at) = next.at_tick {
                let now = world.time.tick;
                sec.line(
                    "Leaves",
                    if at > now {
                        format!("{} · in {}", tick_clock(world, at), span(at - now))
                    } else {
                        "Any moment".into()
                    },
                    Tone::Normal,
                );
            }
            if let Some(k) = km {
                sec.line("By road", k, Tone::Normal);
            }
            sec.line("", "Dashed on the map: the way they will drive", Tone::Muted);
        }
    }
    Some(sec)
}

/// Everything about one person (`routes`: their trip, traced once).
pub fn person_sheet(world: &World, id: CitizenId, routes: &mut super::route::TripCache) -> Option<Sheet> {
    let v = inspect::citizen(world, id)?;
    let c = v.citizen;
    let h = v.household;
    let headline = match c.job {
        Some(j) => format!(
            "{} job · {}",
            capitalise(tier_word(j.tier)),
            building_name(world, j.workplace)
        ),
        None if c.retired => "Retired".into(),
        None if c.homemaker => "Looks after the home".into(),
        None if c.student() => "Student".into(),
        None if c.job_seeker() => "Looking for work".into(),
        None => capitalise(stage_word(c.stage)),
    };
    let mut actions = vec![Link::new("Home", building_name(world, v.home), Target::Place(v.home))];
    if let Some(j) = c.job {
        actions.push(Link::new(
            "Work",
            building_name(world, j.workplace),
            Target::Place(j.workplace),
        ));
    }
    let mood_reason = inspect::happiness_reasons(world, id).and_then(|rs| {
        rs.iter()
            .filter(|r| r.value < 0.6)
            .min_by(|a, b| a.value.total_cmp(&b.value))
            .map(|r| r.part.complaint().to_string())
    });
    let mut s = Sheet {
        kind: SheetKind::Person,
        title: name(world, id),
        headline,
        subtitle: format!("Age {} · household of {}", v.age_years(), v.members.len()),
        trip: v.trip.map(|t| trip_bar(world, &t)),
        mood_reason,
        actions,
        portrait: Some(portrait(c.seed, c.stage)),
        mood: Some(c.happiness.total),
        at: routes
            .person(world, id)
            .and_then(|t| t.at)
            .or_else(|| person_position(world, id)),
        vehicle: v.trip.and_then(|t| t.vehicle),
        inside: v.inside,
        ..Default::default()
    };
    // Now and next.
    let (now, next) = doing(world, id);
    let mut sec = Section::new("Now");
    sec.line("", now, Tone::Normal);
    if !next.is_empty() {
        sec.line("", next, Tone::Muted);
    }
    if let Some(t) = v.trip {
        if trip_unseen(world, &t) {
            sec.line("", unseen_words(&t), Tone::Muted);
        }
        if let Some(car) = t.vehicle.filter(|c| world.vehicles.contains_key(*c)) {
            sec.link("In", "their car", Target::Vehicle(car));
        }
        if let Some(from) = t.from {
            sec.link("From", building_name(world, from), Target::Place(from));
        }
        sec.link("To", building_name(world, t.to), Target::Place(t.to));
    } else if let Some(b) = v.inside
        && b != v.home
    {
        sec.link("At", building_name(world, b), Target::Place(b));
    }
    s.sections.push(sec);
    // The trip on the map: where to, why, how far and how long.
    if let Some(sec) = route_section(world, id, v.trip.as_ref(), s.trip.as_ref(), routes) {
        s.sections.push(sec);
    }
    // Home and work.
    let mut sec = Section::folding("Life", FoldKey::Life);
    sec.link("Home", building_name(world, v.home), Target::Place(v.home));
    match c.job {
        Some(j) => {
            let shift = format!(
                "{}–{}",
                clock(j.shift.start as u64),
                clock((j.shift.start + j.shift.length) as u64 % balance::TICKS_PER_DAY as u64)
            );
            sec.link(
                "Work",
                format!("{} · {shift}", building_name(world, j.workplace)),
                Target::Place(j.workplace),
            );
            sec.line(
                "Wage",
                format!(
                    "{}/day · {}{}",
                    format::money(j.wage as i64),
                    tier_word(j.tier),
                    if j.part_time { ", part time" } else { "" }
                ),
                Tone::Normal,
            );
        }
        None if c.retired => sec.line("Work", "Retired", Tone::Muted),
        None if c.homemaker => sec.line("Work", "Looks after the home", Tone::Muted),
        None if c.student() => sec.line("Work", "Studying", Tone::Muted),
        None if c.job_seeker() => sec.line(
            "Work",
            format!("Looking for a job ({} days)", c.unemployed_days),
            Tone::Warn,
        ),
        None => sec.line(
            "Work",
            if c.stage == LifeStage::Child {
                "Too young"
            } else {
                "None"
            },
            Tone::Muted,
        ),
    }
    if c.last_commute_s > 0.0 && c.job.is_some() {
        sec.line(
            "Last commute",
            format!("{} by car", super::bar::commute_text(c.last_commute_s)),
            Tone::Muted,
        );
    }
    s.sections.push(sec);
    // Household.
    let mut sec = Section::folding(
        format!("{} household · {}", kind_word(h.kind), family_name(h.seed)),
        FoldKey::Household,
    );
    for &m in v.members {
        if m == id {
            continue;
        }
        if let Some(mc) = world.citizens.get(m) {
            let what = match (&mc.location, mc.activity) {
                (citysim_sim::citizens::Location::Travelling(_), _) => {
                    trip_reason(world, m).map_or("on the way".into(), |r| format!("on the way {}", reason_word(r)))
                }
                (_, a) => activity_word(a).to_lowercase(),
            };
            sec.link(
                format!("{} {}", given_name(world, m), mc.age_years()),
                what,
                Target::Person(m),
            );
        }
    }
    let income = citysim_sim::life::daily_income(world, v.household_id);
    sec.line(
        "Money",
        format!(
            "{} · income {}/day · rent {}/day",
            format::money(h.money),
            format::money(income),
            format::money(h.rent)
        ),
        if h.money < 0 { Tone::Bad } else { Tone::Normal },
    );
    if h.austerity {
        sec.line("", "Saving: only buys what they need", Tone::Warn);
    }
    s.sections.push(sec);
    // Happiness with its reasons.
    let mut sec = Section::folding(
        match &s.mood_reason {
            Some(r) => format!("{} · {}", mood_word(c.happiness.total), r.to_lowercase()),
            None => format!(
                "{} · {}",
                mood_word(c.happiness.total),
                format::percent((c.happiness.total * 100.0).round() / 100.0)
            ),
        },
        FoldKey::Happiness,
    );
    if let Some(reasons) = inspect::happiness_reasons(world, id) {
        for r in reasons {
            sec.bar(part_title(r.part), r.text, r.value);
        }
    }
    s.sections.push(sec);
    // Journal, newest first.
    let mut sec = Section::folding(
        match c.journal.last() {
            Some(e) => format!("Journal · {}", journal_words(world, e.event).to_lowercase()),
            None => "Journal".into(),
        },
        FoldKey::Journal,
    );
    for e in c.journal.iter().rev().take(6) {
        sec.line(format!("Day {}", e.day), journal_words(world, e.event), Tone::Muted);
    }
    if c.journal.is_empty() {
        sec.line("", "Nothing yet", Tone::Muted);
    }
    s.sections.push(sec);
    Some(s)
}

/// Who lives or works in a building.
pub fn place_sheet(world: &World, id: BuildingId) -> Option<Sheet> {
    let b = world.buildings.get(id)?;
    let v = inspect::building(world, id)?;
    let mut s = Sheet {
        kind: if b.kind == Zone::Residential {
            SheetKind::Home
        } else {
            SheetKind::Workplace
        },
        title: building_name(world, id),
        headline: match b.kind {
            Zone::Residential => format!("{} households · {} residents", v.households.len(), v.occupied),
            _ => format!("{} of {} jobs filled", v.occupied, v.capacity),
        },
        subtitle: match b.kind {
            Zone::Residential => format!("Homes · level {}", b.level),
            Zone::Commercial => format!("Shops & offices · level {}", b.level),
            Zone::Industrial => format!("Industry · level {}", b.level),
        },
        at: Some(b.center()),
        inside: Some(id),
        ..Default::default()
    };
    let mut sec = Section::new("Overview");
    match b.kind {
        Zone::Residential => {
            sec.bar(
                "Residents",
                format!("{} of {}", v.occupied, v.capacity),
                v.occupied as f32 / v.capacity.max(1) as f32,
            );
            let home = inspect::residents(world, id)
                .filter(|&c| world.citizens.get(c).is_some_and(|c| c.location.building() == Some(id)))
                .count();
            let guests = inspect::inside(world, id).count().saturating_sub(home);
            sec.line(
                "In right now",
                if guests > 0 {
                    format!("{home} at home · {guests} visiting")
                } else {
                    format!("{home} at home")
                },
                Tone::Normal,
            );
            sec.line(
                "Rent",
                format!("{}/day from all households", format::money(v.rent_per_day)),
                Tone::Normal,
            );
        }
        _ => {
            sec.bar(
                "Jobs filled",
                format!("{} of {}", v.occupied, v.capacity),
                v.occupied as f32 / v.capacity.max(1) as f32,
            );
            sec.line(
                "Inside now",
                format!("{} at work · {} visitors", v.staff_present, v.visitors),
                Tone::Normal,
            );
            sec.line("Wages", format!("{}/day", format::money(v.wages_per_day)), Tone::Normal);
            if b.kind == Zone::Commercial && v.stock_cap > 0.0 {
                sec.bar(
                    "Shelves",
                    format!("{:.0} of {:.0} · sold {:.0} today", v.stock, v.stock_cap, v.sold_today),
                    v.stock / v.stock_cap,
                );
            }
        }
    }
    s.sections.push(sec);
    if b.kind == Zone::Residential {
        // One foldable list: each household's name line, then its people.
        let mut sec = Section::folding(
            format!(
                "Who lives here · {} {}",
                v.households.len(),
                if v.households.len() == 1 {
                    "household"
                } else {
                    "households"
                }
            ),
            FoldKey::Residents,
        );
        for &hid in v.households.iter().take(8) {
            let Some(h) = world.households.get(hid) else { continue };
            sec.line(
                format!("{} · {}", family_name(h.seed), kind_word(h.kind).to_lowercase()),
                format!("rent {}/day", format::money(h.rent)),
                Tone::Muted,
            );
            for &m in &h.citizens {
                let Some(c) = world.citizens.get(m) else { continue };
                let here = c.location.building() == Some(id);
                let what = match &c.location {
                    citysim_sim::citizens::Location::Travelling(_) => {
                        trip_reason(world, m).map_or("out".into(), |r| format!("out, {}", reason_word(r)))
                    }
                    _ if here => format!("in · {}", activity_word(c.activity).to_lowercase()),
                    _ => format!("out · {}", activity_word(c.activity).to_lowercase()),
                };
                sec.link(
                    format!("  {} {}", given_name(world, m), c.age_years()),
                    what,
                    Target::Person(m),
                );
            }
        }
        if v.households.len() > 8 {
            sec.line(
                "",
                format!("and {} more households", v.households.len() - 8),
                Tone::Muted,
            );
        }
        s.sections.push(sec);
    } else {
        let mut sec = Section::folding(format!("Who works here · {}", v.workers.len()), FoldKey::Workers);
        let mut rows: Vec<(bool, CitizenId)> = v
            .workers
            .iter()
            .map(|&w| {
                (
                    world.citizens.get(w).is_some_and(|c| c.location.building() == Some(id)),
                    w,
                )
            })
            .collect();
        // Those inside first.
        rows.sort_by_key(|r| !r.0);
        for &(here, w) in rows.iter().take(10) {
            let Some(c) = world.citizens.get(w) else { continue };
            let wage = c.job.map_or(0, |j| j.wage as i64);
            sec.link(
                name(world, w),
                format!("{} · {}/day", if here { "in" } else { "away" }, format::money(wage)),
                Target::Person(w),
            );
        }
        if rows.len() > 10 {
            sec.line("", format!("and {} more", rows.len() - 10), Tone::Muted);
        }
        if rows.is_empty() {
            sec.line("", "Nobody yet", Tone::Muted);
        }
        s.sections.push(sec);
    }
    Some(s)
}

/// A car or truck.
pub fn vehicle_sheet(world: &World, id: VehicleId) -> Option<Sheet> {
    let v = inspect::vehicle(world, id)?;
    let veh = world.vehicles.get(id)?;
    let truck = v.purpose == TripPurpose::Goods;
    let driver_trip = v
        .driver
        .or(v.owner)
        .and_then(|c| inspect::citizen(world, c))
        .and_then(|cv| cv.trip)
        .filter(|t| t.vehicle == Some(id));
    let mut actions = Vec::new();
    if let Some(d) = v.driver.or(v.owner) {
        actions.push(Link::new("Driver", name(world, d), Target::Person(d)));
    }
    let mut s = Sheet {
        kind: SheetKind::Vehicle,
        headline: if truck {
            "Delivering goods".to_string()
        } else {
            format!("Going {} · {} aboard", reason_word(v.reason), v.occupants)
        },
        trip: driver_trip.map(|t| trip_bar(world, &t)),
        actions,
        title: if truck {
            "Delivery truck".into()
        } else if let Some(d) = v.driver.or(v.owner) {
            format!("{}'s car", name(world, d))
        } else {
            "A car".into()
        },
        subtitle: if veh.stopped() {
            "Standing in traffic".into()
        } else {
            format!("Moving at {}", super::context::kmh(veh.speed))
        },
        at: Some(citysim_sim::drive::vehicle_pose(world, veh).0),
        vehicle: Some(id),
        ..Default::default()
    };
    let mut sec = Section::new("Trip");
    let place = |p: Place, b: Option<BuildingId>| match (p, b) {
        (Place::Entry, _) => ("the highway".to_string(), None),
        (_, Some(b)) => (building_name(world, b), Some(b)),
        _ => ("somewhere".to_string(), None),
    };
    for (label, (text, b)) in [
        ("From", place(v.origin, v.origin_building)),
        ("To", place(v.destination, v.destination_building)),
    ] {
        match b {
            Some(b) => sec.link(label, text, Target::Place(b)),
            None => sec.line(label, text, Tone::Normal),
        }
    }
    sec.line("Why", v.reason.text(), Tone::Normal);
    if let Some(pts) = super::route::vehicle_path(world, id) {
        let left = s.trip.as_ref().map(|t| t.left.clone());
        sec.line(
            "Still to go",
            match left {
                Some(l) if !s.trip.as_ref().is_some_and(|t| t.late) => {
                    format!(
                        "{} · about {l}",
                        super::readout::metres(super::route::path_length(&pts))
                    )
                }
                _ => super::readout::metres(super::route::path_length(&pts)),
            },
            Tone::Normal,
        );
    }
    if v.destination_building.is_some() || v.destination == Place::Entry {
        sec.jump("On the map", "Show where it goes");
    }
    // Game time, like every clock in the panel (a real second at 1x is a
    // game minute and a half).
    sec.line(
        "On the way",
        span(world.time.tick.saturating_sub(v.requested_tick)),
        Tone::Normal,
    );
    if truck && v.cargo > 0.0 {
        sec.line("Cargo", format!("{:.0} units of goods", v.cargo), Tone::Normal);
    }
    s.sections.push(sec);
    let mut sec = Section::folding(format!("In the car · {}", v.occupants), FoldKey::Aboard);
    for c in inspect::occupants(world, id) {
        let role = if Some(c) == v.driver { "driving" } else { "passenger" };
        let age = world.citizens.get(c).map_or(0, |c| c.age_years());
        sec.link(name(world, c), format!("{age} · {role}"), Target::Person(c));
    }
    if v.party > 0 {
        sec.line("", format!("{} people leaving the city", v.party), Tone::Muted);
    }
    if v.occupants == 0 {
        sec.line(
            "",
            if truck {
                "A driver from out of town"
            } else {
                "Nobody we know"
            },
            Tone::Muted,
        );
    }
    s.sections.push(sec);
    Some(s)
}

/// The whole city's people.
pub fn city_sheet(world: &World) -> Sheet {
    let st = &world.people.stats;
    let mut s = Sheet {
        kind: SheetKind::City,
        title: "People".into(),
        subtitle: format!(
            "{} residents · {} households · {}",
            format::group(st.population as u64),
            format::group(st.households as u64),
            mood_word(st.avg_happiness).to_lowercase()
        ),
        mood: Some(st.avg_happiness),
        ..Default::default()
    };
    let total = st.population.max(1) as f32;
    // The age pyramid: ten-year bands, oldest at the top.
    let mut sec = Section::new("Ages");
    let most = st.age_pyramid.iter().copied().max().unwrap_or(1).max(1) as f32;
    for b in (0..AGE_BANDS).rev() {
        let n = st.age_pyramid[b];
        let label = if b + 1 == AGE_BANDS {
            format!("{}+", b * 10)
        } else {
            format!("{}–{}", b * 10, b * 10 + 9)
        };
        sec.bar(label, format::group(n as u64), n as f32 / most);
    }
    s.sections.push(sec);
    let mut sec = Section::new("Households");
    let hh = st.households.max(1) as f32;
    for (k, n) in inspect::household_mix(world) {
        sec.bar(kind_word(k), format::group(n as u64), n as f32 / hh);
    }
    s.sections.push(sec);
    let mut sec = Section::new("Work");
    let wf = st.workforce.max(1) as f32;
    let unemployed = st.workforce.saturating_sub(st.employed);
    sec.bar("Employed", format::group(st.employed as u64), st.employed as f32 / wf);
    sec.bar(
        "Looking for work",
        format::group(unemployed as u64),
        unemployed as f32 / wf,
    );
    for (t, n) in inspect::employment_by_tier(world) {
        sec.line(format!("  {}", tier_word(t)), format::group(n as u64), Tone::Muted);
    }
    sec.line("Retired", format::group(st.retired as u64), Tone::Muted);
    sec.line(
        "Looking after the home",
        format::group(st.homemakers as u64),
        Tone::Muted,
    );
    s.sections.push(sec);
    let mut sec = Section::new("Right now");
    let shares = inspect::activity_shares(world);
    for a in Activity::ALL {
        let sh = shares[a.index()];
        sec.bar(activity_word(a), format!("{}", (sh * total).round() as i64), sh);
    }
    s.sections.push(sec);
    let mut sec = Section::new("Trips today");
    let trips = inspect::trips_today(world);
    let most = trips.iter().copied().max().unwrap_or(1).max(1) as f32;
    for p in TripPurpose::ALL {
        let n = trips[p.index()];
        if n > 0 || matches!(p, TripPurpose::ToWork | TripPurpose::ToHome) {
            sec.bar(purpose_title(p), format::group(n as u64), n as f32 / most);
        }
    }
    s.sections.push(sec);
    let mut sec = Section::new(format!(
        "Unhappy · {} of {}",
        format::group(st.unhappy as u64),
        format::group(st.population as u64)
    ));
    let mut any = false;
    for (p, n) in st.top_reasons(4) {
        any = true;
        sec.bar(
            p.complaint(),
            format::group(n as u64),
            n as f32 / st.unhappy.max(1) as f32,
        );
    }
    if !any {
        sec.line("", "Nobody is unhappy", Tone::Good);
    }
    s.sections.push(sec);
    let mut sec = Section::new("Money");
    sec.line("Average savings", format::money(st.avg_money as i64), Tone::Normal);
    sec.line(
        "Average rent",
        format!("{}/day", format::money(st.avg_rent as i64)),
        Tone::Normal,
    );
    sec.line("Rent of income", format::percent(st.avg_rent_burden), Tone::Normal);
    if st.broke > 0 {
        sec.line("Households in debt", format::group(st.broke as u64), Tone::Warn);
    }
    s.sections.push(sec);
    // What happiness is made of, city-wide (1 = content).
    let mut sec = Section::new(format!("Happiness · {}", mood_word(st.avg_happiness).to_lowercase()));
    for p in citysim_sim::citizens::HappinessPart::ALL {
        let v = st.avg_parts[p.index()];
        sec.bar(part_title(p), format!("{}", (v * 100.0).round() as i64), v);
    }
    s.sections.push(sec);
    s
}

/// Short words for a vehicle on the inspect card.
pub fn vehicle_lines(world: &World, id: VehicleId) -> Option<(String, Vec<Line>)> {
    let s = vehicle_sheet(world, id)?;
    let mut lines = Vec::new();
    for sec in &s.sections {
        for it in &sec.items {
            match it {
                Item::Line(l) => lines.push(l.clone()),
                Item::Link(l) => lines.push(Line::new(l.label.clone(), l.value.clone(), Tone::Normal)),
                Item::Bar { .. } | Item::Jump { .. } => {}
            }
        }
    }
    Some((s.title, lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_faces_are_stable_and_varied() {
        assert_eq!(first_name(42), first_name(42));
        let names: std::collections::BTreeSet<&str> = (0..200).map(first_name).collect();
        assert!(names.len() > 40, "{}", names.len());
        let fams: std::collections::BTreeSet<&str> = (0..200).map(family_name).collect();
        assert!(fams.len() > 30);
        let a = portrait(7, LifeStage::Adult);
        assert_eq!(a, portrait(7, LifeStage::Adult));
        assert_eq!(portrait(7, LifeStage::Senior).hair, [0.78, 0.78, 0.80]);
    }

    #[test]
    fn absolute_ticks_read_as_the_time_of_day() {
        let mut w = World::new(1);
        for _ in 0..10 {
            w.step();
        }
        // Whatever tick 0 was, "now" reads as the clock the HUD shows.
        assert_eq!(tick_clock(&w, w.time.tick), clock(w.time.tick_of_day as u64));
        let hour = balance::TICKS_PER_HOUR as u64;
        let later = tick_clock(&w, w.time.tick + 2 * hour);
        assert_eq!(
            later,
            clock((w.time.tick_of_day as u64 + 2 * hour) % balance::TICKS_PER_DAY as u64)
        );
    }

    #[test]
    fn clock_and_spans_read_naturally() {
        let hour = balance::TICKS_PER_HOUR as u64;
        assert_eq!(clock(7 * hour + hour / 2), "07:30");
        assert_eq!(span(hour * 2 + hour / 6), "2 h 10 min");
        assert_eq!(span(hour / 4), "15 min");
    }

    #[test]
    fn sheets_of_a_grown_town() {
        let mut w = World::new(3);
        citysim_sim::scenarios::prebuilt(&mut w, |w| citysim_sim::scenarios::two_districts(w, true));
        for _ in 0..(balance::TICKS_PER_DAY * 2) {
            w.step();
        }
        let (id, _) = w.citizens.iter().find(|(_, c)| c.job.is_some()).expect("a worker");
        let s = person_sheet(&w, id, &mut Default::default()).expect("person");
        assert!(!s.title.is_empty() && s.portrait.is_some());
        assert!(s.sections.iter().any(|s| s.title == "Life"));
        assert!(s.at.is_some());
        let home = w.citizens[id].household;
        let home = w.households[home].home;
        let p = place_sheet(&w, home).expect("home");
        assert!(p.sections.len() >= 2);
        let city = city_sheet(&w);
        assert!(city.sections.iter().any(|s| s.title == "Ages"));
        if let Some((v, _)) = w.vehicles.iter().next() {
            assert!(vehicle_sheet(&w, v).is_some());
        }
    }
}
