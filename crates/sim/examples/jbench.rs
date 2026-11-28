//! Junction capacity bench (`citysim_sim::capacity`): cars per minute
//! through one junction of each kind, saturated from every approach.
//!
//! `cargo run -p citysim_sim --release --example jbench [csv]`
//! `cargo run -p citysim_sim --release --example jbench rings [csv]`: give-way,
//! signals and every roundabout size at a street crossing, an avenue
//! crossing a street and two avenues, at 10, 30 and 50 % turning (half
//! left, half right): cars per minute with every approach saturated, and
//! the mean wait per car at a moderate demand.

use citysim_sim::capacity::{Bench, Load, run};
use citysim_sim::{JunctionSetting as S, RoadKind as K};

fn rings(csv: Option<String>) {
    use citysim_sim::RoundaboutSize as R;
    let roads = [
        ("street x street", K::Street, K::Street, (6.0, 6.0)),
        ("avenue x street", K::Avenue, K::Street, (18.0, 6.0)),
        ("avenue x avenue", K::Avenue, K::Avenue, (20.0, 20.0)),
    ];
    let controls: [(&str, S, Option<R>); 6] = [
        ("give-way", S::Priority, None),
        ("signals", S::Signal, None),
        ("mini", S::Priority, Some(R::Mini)),
        ("small", S::Priority, Some(R::Small)),
        ("medium", S::Priority, Some(R::Medium)),
        ("large", S::Priority, Some(R::Large)),
    ];
    // `JBENCH_WALKERS=n`: people a minute walking past every junction.
    let walkers: f32 = std::env::var("JBENCH_WALKERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let mut out = String::from("roads,turning,control,saturated_per_min,cross_share,gave_up,moderate_wait_s\n");
    for (name, main, cross, (rm, rc)) in roads {
        for turning in [0.1f32, 0.3, 0.5] {
            for (cname, setting, ring) in controls {
                let mut b = Bench::crossing(main, cross, setting)
                    .turning(turning * 0.5, turning * 0.5)
                    .walkers(walkers);
                if let Some(r) = ring {
                    let (w, n) = citysim_sim::capacity::build(&Bench::crossing(main, cross, setting));
                    if n.is_some_and(|n| citysim_sim::roundabout::sizes(&w, n)[r as usize].1.is_err()) {
                        println!("{name:16} {:3.0}% {cname:9} does not fit", turning * 100.0);
                        continue;
                    }
                    b = b.roundabout(r);
                }
                let sat = run(&b, 7, 120, 600);
                let moderate = run(&b.clone().loads(Load::Rate(rm), Load::Rate(rc)), 7, 120, 600);
                let share = sat.cross_per_minute / sat.per_minute.max(1e-3);
                println!(
                    "{name:16} {:3.0}% {cname:9} saturated {:6.1}/min (side roads {:4.0}%, gave up {:3})  moderate: wait {:5.1} s",
                    turning * 100.0,
                    sat.per_minute,
                    share * 100.0,
                    sat.gave_up,
                    moderate.mean_wait_s
                );
                out.push_str(&format!(
                    "{name},{turning},{cname},{:.1},{:.3},{},{:.2}\n",
                    sat.per_minute, share, sat.gave_up, moderate.mean_wait_s
                ));
            }
        }
    }
    if let Some(p) = csv {
        std::fs::write(p, out).unwrap();
    }
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("rings") {
        rings(std::env::args().nth(2));
        return;
    }
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
