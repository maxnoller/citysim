//! Renders a top-down video of the morning rush in two cities with the same
//! zoning: a grid of streets (left) and everything off one junction (right).
//! Roads are coloured by congestion (grey free, yellow busy, red jammed),
//! cars are white dots on their lane, buildings by zone and level.
//!
//! Writes raw RGB frames to stdout; pipe into ffmpeg:
//!
//! ```text
//! cargo run -p citysim_sim --release --example traffic_video | ffmpeg -f rawvideo \
//!     -pix_fmt rgb24 -s 1280x720 -r 30 -i - -c:v libx264 -pix_fmt yuv420p out.mp4
//! ```

use std::io::Write;

use citysim_sim::*;

const W: usize = 1280;
const H: usize = 720;
/// Map window shown in each half, cells.
const X0: f32 = 8.0;
const Y0: f32 = 94.0;
const SPAN_X: f32 = 84.0;
const SCALE: f32 = (W as f32 / 2.0 - 16.0) / (SPAN_X * CELL_SIZE);
const TOP: usize = 120;

struct Canvas {
    px: Vec<u8>,
}

impl Canvas {
    fn put(&mut self, x: i32, y: i32, c: [u8; 3]) {
        if x >= 0 && y >= 0 && (x as usize) < W && (y as usize) < H {
            let i = (y as usize * W + x as usize) * 3;
            self.px[i..i + 3].copy_from_slice(&c);
        }
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: [u8; 3]) {
        for yy in y.round() as i32..(y + h).round() as i32 {
            for xx in x.round() as i32..(x + w).round() as i32 {
                self.put(xx, yy, c);
            }
        }
    }
}

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    [0, 1, 2].map(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t) as u8)
}

fn congestion_color(x: f32) -> [u8; 3] {
    let free = [120, 124, 130];
    let busy = [235, 180, 40];
    let jam = [220, 45, 40];
    if x < 0.5 {
        mix(free, busy, x / 0.5)
    } else {
        mix(busy, jam, (x - 0.5) / 0.4)
    }
}

fn draw(c: &mut Canvas, w: &World, ox: f32) {
    let to_px = |p: [f32; 2]| {
        (
            ox + (p[0] - X0 * CELL_SIZE) * SCALE,
            TOP as f32 + (p[1] - Y0 * CELL_SIZE) * SCALE,
        )
    };
    let cell = CELL_SIZE * SCALE;
    // Zones and buildings.
    for y in Y0 as i32..(Y0 + 80.0) as i32 {
        for x in X0 as i32..(X0 + SPAN_X) as i32 {
            let cd = w.grid.get(Cell::new(x, y)).unwrap();
            let (px, py) = to_px([x as f32 * CELL_SIZE, y as f32 * CELL_SIZE]);
            if py + cell > H as f32 {
                continue;
            }
            let col = match cd.occupant {
                Occupant::Building(id) => {
                    let b = &w.buildings[id];
                    let base = match b.kind {
                        Zone::Residential => [70, 150, 85],
                        Zone::Commercial => [70, 120, 200],
                        Zone::Industrial => [200, 160, 60],
                    };
                    let lit = 0.15 * (b.level as f32 - 1.0) + 0.25 * (b.occupied as f32 / b.capacity.max(1) as f32);
                    Some(mix(base, [255, 255, 255], lit * 0.6))
                }
                Occupant::Empty => cd.zone.map(|z| match z {
                    Zone::Residential => [38, 58, 42],
                    Zone::Commercial => [36, 48, 66],
                    Zone::Industrial => [64, 56, 36],
                }),
                Occupant::Road => None,
            };
            if let Some(col) = col {
                c.rect(px + 1.0, py + 1.0, cell - 2.0, cell - 2.0, col);
            }
        }
    }
    // Roads, coloured per direction by congestion (each direction on its side).
    for (s, seg) in w.roads.segments.iter() {
        let (a, b) = w.roads.segment_ends(s);
        let d = [b[0] - a[0], b[1] - a[1]];
        let len = seg.length.max(1e-3);
        let n = [-d[1] / len, d[0] / len];
        let steps = (len * SCALE * 1.5) as usize + 1;
        for (dir, side) in [(true, 1.0f32), (false, -1.0f32)] {
            let col = congestion_color(seg.congestion_in(dir));
            for k in 0..=steps {
                let t = k as f32 / steps as f32;
                let p = [a[0] + d[0] * t + n[0] * side * 1.6, a[1] + d[1] * t + n[1] * side * 1.6];
                let (x, y) = to_px(p);
                if y < TOP as f32 || y >= H as f32 - 2.0 {
                    continue;
                }
                c.rect(x - 1.5, y - 1.5, 3.0, 3.0, col);
            }
        }
    }
    // Cars on their lane (right-hand traffic: offset to the right of heading).
    for v in w.vehicles.values() {
        let (s, co) = v.heading.sin_cos();
        let p = [v.pos[0] - s * -1.6, v.pos[1] + co * -1.6];
        let (x, y) = to_px(p);
        if y < TOP as f32 || y >= H as f32 - 2.0 {
            continue;
        }
        let col = if v.speed < 1.0 {
            [255, 120, 120]
        } else {
            [250, 250, 250]
        };
        c.rect(x - 1.5, y - 1.5, 3.0, 3.0, col);
    }
}

fn main() {
    let start_day = 6u64;
    let mut good = World::new(8);
    citysim_sim::scenarios::prebuilt(&mut good, |w| citysim_sim::scenarios::two_districts(w, true));
    let mut bad = World::new(8);
    citysim_sim::scenarios::prebuilt(&mut bad, |w| citysim_sim::scenarios::two_districts(w, false));
    let start = (start_day - 1) * balance::TICKS_PER_DAY as u64 + balance::TICKS_PER_HOUR as u64; // 07:00
    while good.time.tick < start {
        good.step();
        bad.step();
    }
    let frames = 30 * 20;
    let steps_per_frame = 2;
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    let mut stats = String::new();
    for f in 0..frames {
        for _ in 0..steps_per_frame {
            good.step();
            bad.step();
        }
        let mut c = Canvas { px: vec![0; W * H * 3] };
        c.rect(0.0, 0.0, W as f32, H as f32, [24, 27, 31]);
        draw(&mut c, &good, 8.0);
        draw(&mut c, &bad, W as f32 / 2.0 + 8.0);
        c.rect(W as f32 / 2.0 - 1.0, 0.0, 2.0, H as f32, [60, 64, 70]);
        out.write_all(&c.px).unwrap();
        if f % 30 == 0 {
            for (name, w) in [("good", &good), ("bad", &bad)] {
                stats.push_str(&format!(
                    "{} {:02}:{:02} {name}: pop {} cars {} speed {:.0} km/h commute {:.0} s jam {:.0}%\n",
                    f / 30,
                    w.time.hour() as u32,
                    (w.time.hour().fract() * 60.0) as u32,
                    w.stats.population,
                    w.stats.vehicles_on_road,
                    w.stats.avg_speed * 3.6,
                    w.stats.avg_commute_s,
                    w.stats.congestion_share * 100.0
                ));
            }
        }
    }
    eprint!("{stats}");
}
