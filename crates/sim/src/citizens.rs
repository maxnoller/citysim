//! People: every resident of the city is a [`Citizen`].
//!
//! A citizen belongs to exactly one [`crate::Household`], has an age on the
//! compressed calendar (see `balance::DAYS_PER_YEAR`), a life stage, maybe a
//! job of their own, and is always in exactly one place: inside a building
//! ([`Location::In`]) or travelling ([`Location::Travelling`], on screen in a
//! car or off screen with a known arrival time).
//!
//! The rules that move citizens around live in `day` (their daily
//! activities and trips), `life` (ageing, life events, money, happiness,
//! migration) and `people` (bookkeeping shared by both). Read access for
//! the interface is in [`crate::inspect`].

use serde::{Deserialize, Serialize};
use slotmap::new_key_type;

use crate::traffic::TripPurpose;
use crate::{BuildingId, HouseholdId, VehicleId, balance};

new_key_type! {
    /// Stable id of a [`Citizen`].
    pub struct CitizenId;
}

/// Stage of life, from the age (see `balance::{TEEN_AGE_YEARS,
/// ADULT_AGE_YEARS, SENIOR_AGE_YEARS}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub enum LifeStage {
    /// 0-12 years. Lives at home, travels only as a passenger.
    #[default]
    Child,
    /// 13-17 years. May take a part-time job in a shop.
    Teen,
    /// 18-64 years. In the workforce (unless a stay-at-home parent).
    Adult,
    /// 65 years and older. Retired, lives on a pension.
    Senior,
}

impl LifeStage {
    pub const ALL: [LifeStage; 4] = [LifeStage::Child, LifeStage::Teen, LifeStage::Adult, LifeStage::Senior];

    /// Stage for an age in whole years.
    pub fn of_years(years: u32) -> LifeStage {
        if years >= balance::SENIOR_AGE_YEARS {
            LifeStage::Senior
        } else if years >= balance::ADULT_AGE_YEARS {
            LifeStage::Adult
        } else if years >= balance::TEEN_AGE_YEARS {
            LifeStage::Teen
        } else {
            LifeStage::Child
        }
    }

    /// Index into per-stage arrays (`ALL` order).
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Highest education reached. Not used yet: schools and education tiers
/// come in stage 2 (every citizen is `None` for now).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub enum Education {
    #[default]
    None,
    Primary,
    Secondary,
    Higher,
}

/// What a citizen is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub enum Activity {
    /// Asleep at home.
    Sleeping,
    /// Awake at home (staying in).
    #[default]
    Home,
    /// At their workplace during their shift.
    Working,
    /// In a shop buying goods for the household.
    Shopping,
    /// Out for leisure (restaurant, cinema... any commercial building).
    Leisure,
    /// On the way somewhere ([`Location::Travelling`]).
    Travelling,
}

impl Activity {
    pub const ALL: [Activity; 6] = [
        Activity::Sleeping,
        Activity::Home,
        Activity::Working,
        Activity::Shopping,
        Activity::Leisure,
        Activity::Travelling,
    ];
    pub const COUNT: usize = 6;

    /// Index into per-activity arrays (`ALL` order).
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Pay grade of a job, from the workplace's kind and level
/// (`people::job_tier`). Wages per day: `balance::WAGE_BY_TIER`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub enum JobTier {
    /// Small shops and sheds (level 1).
    #[default]
    Basic,
    /// Bigger shops, workshops and factories (level 2).
    Skilled,
    /// Offices (commercial level 3).
    Professional,
}

impl JobTier {
    pub const ALL: [JobTier; 3] = [JobTier::Basic, JobTier::Skilled, JobTier::Professional];

    pub fn index(self) -> usize {
        self as usize
    }
}

/// A worker's shift: daily start and length, and the weekdays off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Shift {
    /// Start, ticks after midnight (`0..TICKS_PER_DAY`).
    pub start: u32,
    /// Length in ticks; a shift may run past midnight (night shifts).
    pub length: u32,
    /// Bit `d` set: weekday `d` (`day % 7`) is a day off.
    pub days_off: u8,
}

impl Shift {
    /// Works on the given day number?
    pub fn works_on(&self, day: u32) -> bool {
        self.days_off & (1 << (day % balance::DAYS_PER_WEEK)) == 0
    }

    /// Start hour (0..24), for display.
    pub fn start_hour(&self) -> f32 {
        self.start as f32 / balance::TICKS_PER_HOUR as f32
    }

    /// Length in hours, for display.
    pub fn hours(&self) -> f32 {
        self.length as f32 / balance::TICKS_PER_HOUR as f32
    }
}

/// A citizen's job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Job {
    /// Commercial or industrial building they work in.
    pub workplace: BuildingId,
    pub tier: JobTier,
    /// Gross wage per day worked or not (salaried), dollars. The city's
    /// residential tax is taken from it.
    pub wage: u32,
    /// Teens' part-time jobs.
    pub part_time: bool,
    pub shift: Shift,
    /// Day the job started.
    pub since_day: u32,
}

/// How a trip is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum TravelMode {
    /// Driving one of the household's cars (on screen or off screen).
    #[default]
    Driving,
    /// Riding with a household member who drives (children, car-sharing
    /// partners).
    Passenger,
    /// No car available: the trip happens off screen and takes
    /// `balance::CARLESS_TIME_FACTOR` times as long (a lift, a bus, a long
    /// walk; pedestrians and transit come in stage 3).
    NoCar,
}

/// A trip in progress.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Journey {
    pub purpose: TripPurpose,
    /// Building the trip started at; `None` when arriving from outside the
    /// city (moving in through the highway).
    pub from: Option<BuildingId>,
    /// Building the trip goes to.
    pub to: BuildingId,
    pub mode: TravelMode,
    /// For passengers: the household member driving.
    pub driver: Option<CitizenId>,
    /// The car on the road carrying the citizen (driver or passenger), once
    /// it has started. `None` while the car waits to start, and for trips
    /// made off screen.
    pub vehicle: Option<VehicleId>,
    /// Tick the trip started.
    pub departed_tick: u64,
    /// Off-screen trips: tick of arrival. `None` for trips by a car on the
    /// road (or waiting to start), which end when the car arrives.
    pub arrive_tick: Option<u64>,
}

/// Where a citizen is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Location {
    /// Inside a building (home, workplace, shop or leisure venue).
    In(BuildingId),
    /// On the way.
    Travelling(Journey),
}

impl Location {
    /// The building they are in, if any.
    pub fn building(&self) -> Option<BuildingId> {
        match self {
            Location::In(b) => Some(*b),
            Location::Travelling(_) => None,
        }
    }

    pub fn journey(&self) -> Option<&Journey> {
        match self {
            Location::In(_) => None,
            Location::Travelling(j) => Some(j),
        }
    }
}

/// Place of a citizen in their household.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum HouseholdRole {
    /// One of the adults who run the household (or the senior(s)).
    #[default]
    Head,
    /// A child of the household (born here or arrived as a child); stays a
    /// dependant until they move out, even once grown up.
    Dependant,
}

/// Happiness and what it is made of, each `0.0..=1.0` (1 = content).
/// `total` is the weighted mean (`balance::HAPPINESS_WEIGHTS`, in the field
/// order below). Updated once a day.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Happiness {
    pub total: f32,
    /// Has a job (workers), how long without one; the household's earners
    /// for dependants; pensioners are content.
    pub employment: f32,
    /// Duration of their last commute (workers).
    pub commute: f32,
    /// Money left after rent, and savings.
    pub affordability: f32,
    /// Shops and leisure near home, goods in the cupboard, leisure need met.
    pub access: f32,
    /// The tax rate.
    pub taxes: f32,
    /// City services and debt.
    pub finances: f32,
    /// A home of one's own (grown-up children who cannot move out are not
    /// happy).
    pub housing: f32,
}

impl Default for Happiness {
    fn default() -> Self {
        Happiness {
            total: balance::HAPPINESS_START,
            employment: balance::HAPPINESS_START,
            commute: balance::HAPPINESS_START,
            affordability: balance::HAPPINESS_START,
            access: balance::HAPPINESS_START,
            taxes: balance::HAPPINESS_START,
            finances: balance::HAPPINESS_START,
            housing: balance::HAPPINESS_START,
        }
    }
}

/// One component of [`Happiness`] (for "why are people unhappy").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum HappinessPart {
    Employment,
    Commute,
    Affordability,
    Access,
    Taxes,
    Finances,
    Housing,
}

impl HappinessPart {
    pub const ALL: [HappinessPart; 7] = [
        HappinessPart::Employment,
        HappinessPart::Commute,
        HappinessPart::Affordability,
        HappinessPart::Access,
        HappinessPart::Taxes,
        HappinessPart::Finances,
        HappinessPart::Housing,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Short label of what is wrong when this part is low.
    pub fn complaint(self) -> &'static str {
        match self {
            HappinessPart::Employment => "No job",
            HappinessPart::Commute => "Long commute",
            HappinessPart::Affordability => "Rent and costs too high",
            HappinessPart::Access => "Few shops or leisure nearby",
            HappinessPart::Taxes => "Taxes too high",
            HappinessPart::Finances => "City services cut",
            HappinessPart::Housing => "No home of their own",
        }
    }
}

impl Happiness {
    /// Value of one component.
    pub fn part(&self, p: HappinessPart) -> f32 {
        match p {
            HappinessPart::Employment => self.employment,
            HappinessPart::Commute => self.commute,
            HappinessPart::Affordability => self.affordability,
            HappinessPart::Access => self.access,
            HappinessPart::Taxes => self.taxes,
            HappinessPart::Finances => self.finances,
            HappinessPart::Housing => self.housing,
        }
    }

    /// The component pulling the total down the most (lowest weighted
    /// shortfall), if any is below 1.
    pub fn worst(&self) -> Option<HappinessPart> {
        let w = balance::HAPPINESS_WEIGHTS;
        let mut best: Option<(f32, HappinessPart)> = None;
        for p in HappinessPart::ALL {
            let loss = (1.0 - self.part(p)) * w[p.index()];
            if loss > 0.0 && best.is_none_or(|b| loss > b.0) {
                best = Some((loss, p));
            }
        }
        best.map(|b| b.1)
    }

    /// Recompute `total` from the components.
    pub fn recompute_total(&mut self) {
        let w = balance::HAPPINESS_WEIGHTS;
        let sum: f32 = w.iter().sum();
        let t: f32 = HappinessPart::ALL.iter().map(|&p| self.part(p) * w[p.index()]).sum();
        self.total = (t / sum).clamp(0.0, 1.0);
    }
}

/// A dated event in a citizen's life, for the "follow this person" panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LifeEvent {
    /// Born in the city.
    Born,
    /// Moved to the city (with their household).
    MovedIn,
    /// Started a job at this workplace.
    GotJob(BuildingId),
    /// Lost their job (workplace closed, unreachable, or quit).
    LostJob,
    /// A child was born to their household.
    ChildBorn(CitizenId),
    /// Turned 13 / 18.
    BecameTeen,
    BecameAdult,
    /// Moved out of their parents' home into one of their own.
    MovedOut,
    /// Moved in with a partner.
    Partner(CitizenId),
    /// Their household moved to a cheaper home in the city.
    MovedHome,
    /// Retired at 65.
    Retired,
    /// A member of their household died.
    Bereaved(CitizenId),
}

/// One journal line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Day number (`SimTime::day`) it happened.
    pub day: u32,
    pub event: LifeEvent,
}

/// A resident.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Citizen {
    pub household: HouseholdId,
    pub role: HouseholdRole,
    /// Age in in-game days (`balance::DAYS_PER_YEAR` days make a year).
    pub age_days: u32,
    pub stage: LifeStage,
    /// Stable random number: the interface derives name and appearance from
    /// it; the sim derives personal habits (wake-up time...) from it.
    pub seed: u32,
    /// Unused until stage 2 (schools).
    pub education: Education,
    pub job: Option<Job>,
    /// Retired (seniors).
    pub retired: bool,
    /// Stays at home to look after young children instead of working (not
    /// in the workforce).
    pub homemaker: bool,
    pub activity: Activity,
    pub location: Location,
    /// Tick at which the current activity ends and the citizen next decides
    /// what to do (decisions are made in rotating buckets, so a little
    /// later than this).
    pub until_tick: u64,
    /// Need for leisure, `0.0..=1.0` (1 = badly needs a night out). Grows
    /// while awake, faster at work; restored by leisure.
    pub leisure_need: f32,
    /// Tick `leisure_need` was last brought up to date.
    pub need_tick: u64,
    /// Holds one of the household's cars (driving it, or parked where they
    /// are). Cars come back to the household pool when they get home.
    pub has_car: bool,
    pub happiness: Happiness,
    /// Recent life events, oldest first, at most `balance::JOURNAL_CAP`.
    pub journal: Vec<JournalEntry>,
    /// Duration of their last commute (vehicle seconds; estimated for trips
    /// made off screen). 0 = none yet.
    pub last_commute_s: f32,
    /// Consecutive commutes over `balance::HOPELESS_COMMUTE_S`.
    pub hopeless_commutes: u8,
    /// Consecutive midnights in the workforce without a job.
    pub unemployed_days: u16,
    /// Last day they set off to work.
    pub worked_day: u32,
    /// Day they arrived in the city or were born.
    pub arrived_day: u32,
}

impl Citizen {
    /// Age in whole years.
    pub fn age_years(&self) -> u32 {
        self.age_days / balance::DAYS_PER_YEAR
    }

    /// Wants (or has) a job: adults who are not stay-at-home parents, and
    /// teens who have a part-time job. `Stats::workforce` counts these.
    pub fn in_workforce(&self) -> bool {
        match self.stage {
            LifeStage::Adult => !self.homemaker,
            LifeStage::Teen => self.job.is_some(),
            _ => false,
        }
    }

    /// Looks for a job when it has none: workforce adults, and teens old
    /// enough for a part-time job.
    pub fn job_seeker(&self) -> bool {
        self.job.is_none()
            && match self.stage {
                LifeStage::Adult => !self.homemaker,
                LifeStage::Teen => self.age_years() >= balance::TEEN_JOB_AGE_YEARS,
                _ => false,
            }
    }

    /// Can drive a car (adults and seniors).
    pub fn drives(&self) -> bool {
        matches!(self.stage, LifeStage::Adult | LifeStage::Senior)
    }

    /// Workplace, if employed.
    pub fn workplace(&self) -> Option<BuildingId> {
        self.job.map(|j| j.workplace)
    }

    /// Add a journal line, dropping the oldest beyond the cap.
    pub fn log(&mut self, day: u32, event: LifeEvent) {
        if self.journal.len() >= balance::JOURNAL_CAP {
            self.journal.remove(0);
        }
        self.journal.push(JournalEntry { day, event });
    }
}
