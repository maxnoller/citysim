//! Junction capacity bench (`citysim_sim::capacity`): cars per minute
//! through one junction of each kind, saturated from every approach.
//!
//! `cargo run -p citysim_sim --release --example jbench [csv]`

use citysim_sim::capacity::{Bench, Load, run};
use citysim_sim::{JunctionSetting as S, RoadKind as K};

fn main() {
    let csv = std::env::args().nth(1).filter(|a| !a.starts_with("only="));
    let only: Option<String> = std::env::args().find_map(|a| a.strip_prefix("only=").map(str::to_string));
    let cases: Vec<(&str, Bench)> = vec![
        (
            "street x street all-way",
            Bench::crossing(K::Street, K::Street, S::Priority),
        ),
        (
            "street x street all-way 30% turn",
            Bench::crossing(K::Street, K::Street, S::Priority).turning(0.15, 0.15),
        ),
        (
            "street T street priority",
            Bench::crossing(K::Street, K::Street, S::Priority)
                .tee()
                .turning(0.15, 0.15),
        ),
        (
            "street x street signal",
            Bench::crossing(K::Street, K::Street, S::Signal),
        ),
        (
            "street x street signal 30% turn",
            Bench::crossing(K::Street, K::Street, S::Signal).turning(0.15, 0.15),
        ),
        (
            "avenue x street priority",
            Bench::crossing(K::Avenue, K::Street, S::Priority),
        ),
        (
            "avenue x street priority 30% turn",
            Bench::crossing(K::Avenue, K::Street, S::Priority).turning(0.15, 0.15),
        ),
        (
            "avenue x street signal",
            Bench::crossing(K::Avenue, K::Street, S::Signal),
        ),
        (
            "avenue x street signal 30% turn",
            Bench::crossing(K::Avenue, K::Street, S::Signal).turning(0.15, 0.15),
        ),
        (
            "avenue x avenue signal",
            Bench::crossing(K::Avenue, K::Avenue, S::Signal),
        ),
        (
            "avenue x avenue signal 30% turn",
            Bench::crossing(K::Avenue, K::Avenue, S::Signal).turning(0.15, 0.15),
        ),
        (
            "avenue x avenue signal 15% right",
            Bench::crossing(K::Avenue, K::Avenue, S::Signal).turning(0.0, 0.15),
        ),
        (
            "avenue x avenue signal 15% left",
            Bench::crossing(K::Avenue, K::Avenue, S::Signal).turning(0.15, 0.0),
        ),
        (
            "avenue x avenue signal 10/10",
            Bench::crossing(K::Avenue, K::Avenue, S::Signal).turning(0.10, 0.10),
        ),
        (
            "street x street signal 10/10",
            Bench::crossing(K::Street, K::Street, S::Signal).turning(0.10, 0.10),
        ),
        (
            "street x street signal 15% right",
            Bench::crossing(K::Street, K::Street, S::Signal).turning(0.0, 0.15),
        ),
        (
            "avenue T street signal",
            Bench::crossing(K::Avenue, K::Street, S::Signal)
                .tee()
                .turning(0.15, 0.15),
        ),
        (
            "avenue x street priority (side 6/min)",
            Bench::crossing(K::Avenue, K::Street, S::Priority)
                .loads(Load::Saturated, Load::Rate(6.0))
                .turning(0.15, 0.15),
        ),
    ];
    let mut out = String::from("case,total,main,cross,green_flow_per_lane,main_green_share,gave_up\n");
    for (name, b) in &cases {
        if only.as_ref().is_some_and(|o| name != o) {
            continue;
        }
        let r = run(b, 7, 120, 600);
        println!(
            "{name:40} {:6.1}/min  main {:6.1}  cross {:6.1}  by arm {:?}  green/lane {:?}  green share {:?}  gave up {}  {:?} lanes {:?} turns {:?}",
            r.per_minute,
            r.main_per_minute,
            r.cross_per_minute,
            r.by_arm.map(|x| (x * 10.0).round() / 10.0),
            r.green_flow_per_lane.map(|x| (x * 100.0).round() / 100.0),
            r.main_green_share.map(|x| (x * 100.0).round() / 100.0),
            r.gave_up,
            r.control,
            r.main_by_lane.map(|x| x.round()),
            r.by_turn.map(|x| x.round())
        );
        out.push_str(&format!(
            "{name},{:.1},{:.1},{:.1},{},{},{}\n",
            r.per_minute,
            r.main_per_minute,
            r.cross_per_minute,
            r.green_flow_per_lane.map_or(String::new(), |x| format!("{x:.3}")),
            r.main_green_share.map_or(String::new(), |x| format!("{x:.3}")),
            r.gave_up
        ));
    }
    if let Some(p) = csv {
        std::fs::write(p, out).unwrap();
    }
}
