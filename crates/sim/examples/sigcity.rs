//! A signal of a saved city second by second: `sigcity <save> <x m> <y m>
//! <hour> [seconds]`: phase, stage, the stopped cars per arm, and at each
//! green's end how long it ran.
use citysim_sim::*;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut w = World::load(&std::fs::read(&a[0]).unwrap()).unwrap();
    let p = [a[1].parse::<f32>().unwrap(), a[2].parse::<f32>().unwrap()];
    let hour: f32 = a[3].parse().unwrap();
    let secs: u32 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(300);
    let target = (hour * balance::TICKS_PER_HOUR as f32) as u32;
    let day = w.time.day;
    while w.time.day == day && w.time.tick_of_day < target {
        w.step();
    }
    let n = w
        .roads
        .nodes
        .iter()
        .min_by(|x, y| geom::dist(x.1.pos, p).total_cmp(&geom::dist(y.1.pos, p)))
        .unwrap()
        .0;
    {
        let j = junction::junction(&w, n).unwrap();
        let plan = j.plan.as_ref().unwrap();
        println!("control {:?}", j.control);
        for (i, ph) in plan.phases.iter().enumerate() {
            println!(
                "phase {i}: arms {:?} left_only {} green {}..{} s",
                ph.arms, ph.left_only, ph.min_green_s, ph.max_green_s
            );
        }
        for (i, arm) in j.arms.iter().enumerate() {
            println!("arm {i}: {:?} lanes in {} dir {:?}", arm.kind, arm.lanes_in, arm.dir);
        }
    }
    let mut green_start = 0u32;
    let mut last = (u8::MAX, junction::SignalStage::AllRed);
    let mut through = 0u32;
    let mut greens: Vec<(u8, f32)> = Vec::new();
    for t in 0..secs * TICKS_PER_SECOND {
        w.step();
        let j = junction::junction(&w, n).unwrap();
        let s = j.signal;
        if (s.phase, s.stage) != last {
            if last.1 == junction::SignalStage::Green {
                greens.push((last.0, (t - green_start) as f32 / TICKS_PER_SECOND as f32));
            }
            if s.stage == junction::SignalStage::Green {
                green_start = t;
            }
            last = (s.phase, s.stage);
        }
        through = j.entered_minute as u32;
        if t % TICKS_PER_SECOND == 0 {
            println!(
                "{:4}s phase {} {:?} queue {:?} in-minute {}",
                t / TICKS_PER_SECOND,
                s.phase,
                s.stage,
                j.queue,
                through
            );
            if let Some(arm) = std::env::var("ARM").ok().and_then(|a| a.parse::<usize>().ok()) {
                // Front cars per lane on that arm, heading for the node.
                let seg = j.arms[arm].segment;
                let sg = &w.roads.segments[seg];
                let fwd = sg.b == n;
                let mut front: std::collections::BTreeMap<u8, (f32, VehicleId)> = Default::default();
                for (vid, v) in w.vehicles.iter() {
                    if v.segment == Some(seg) && v.forward == fwd && v.junction.is_none() {
                        let e = front.entry(v.lane).or_insert((v.progress, vid));
                        if v.progress > e.0 {
                            *e = (v.progress, vid);
                        }
                    }
                }
                for (lane, (prog, vid)) in front {
                    let v = &w.vehicles[vid];
                    let next = v.route.first().map(|s| {
                        let o = &w.roads.segments[*s];
                        let other = if o.a == n { o.b } else { o.a };
                        w.roads.nodes[other].pos
                    });
                    println!(
                        "      lane {lane} front {:.1} m to end, speed {:.1}, next toward {:?}, denial {}",
                        sg.length - prog,
                        v.speed,
                        next,
                        traffic::debug_last_denial(&w, vid)
                    );
                }
            }
        }
    }
    let _ = through;
    println!("greens (phase, s): {greens:?}");
    let np = greens.iter().map(|g| g.0).max().unwrap_or(0) as usize + 1;
    for ph in 0..np {
        let v: Vec<f32> = greens.iter().filter(|g| g.0 as usize == ph).map(|g| g.1).collect();
        if !v.is_empty() {
            println!(
                "phase {ph}: {} greens, mean {:.1} s",
                v.len(),
                v.iter().sum::<f32>() / v.len() as f32
            );
        }
    }
}
