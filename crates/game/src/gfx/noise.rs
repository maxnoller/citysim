//! Tiny deterministic hashing and value noise for visual variation
//! (ground colour, tree scatter, building variants). Never used for rules.

/// 32-bit integer hash (lowbias32).
pub fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

pub fn hash2(x: i32, y: i32, seed: u32) -> u32 {
    hash((x as u32).wrapping_mul(0x9E37_79B1) ^ hash((y as u32).wrapping_add(seed.wrapping_mul(0x85EB_CA77))))
}

/// Uniform float in `[0, 1)` from a hash.
pub fn unit(h: u32) -> f32 {
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// A small stream of random numbers from one seed.
pub struct Stream(u32);

impl Stream {
    pub fn new(seed: u32) -> Stream {
        Stream(hash(seed ^ 0xA511_E9B3))
    }

    pub fn next_u32(&mut self) -> u32 {
        self.0 = hash(self.0.wrapping_add(0x9E37_79B9));
        self.0
    }

    /// Uniform in `[0, 1)`.
    pub fn f(&mut self) -> f32 {
        unit(self.next_u32())
    }

    /// Uniform in `[a, b)`.
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }

    /// Uniform integer in `0..n` (`n > 0`).
    pub fn below(&mut self, n: u32) -> u32 {
        self.next_u32() % n.max(1)
    }

    pub fn chance(&mut self, p: f32) -> bool {
        self.f() < p
    }

    pub fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u32) as usize]
    }
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Smooth value noise in `[0, 1]`.
pub fn value(x: f32, y: f32, seed: u32) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (smooth(x - xi as f32), smooth(y - yi as f32));
    let v = |dx: i32, dy: i32| unit(hash2(xi + dx, yi + dy, seed));
    let a = v(0, 0) + (v(1, 0) - v(0, 0)) * fx;
    let b = v(0, 1) + (v(1, 1) - v(0, 1)) * fx;
    a + (b - a) * fy
}

/// Fractal value noise in `[0, 1]`.
pub fn fbm(x: f32, y: f32, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut f) = (0.0, 1.0, 0.0, 1.0);
    for o in 0..octaves {
        sum += value(x * f, y * f, seed.wrapping_add(o * 101)) * amp;
        norm += amp;
        amp *= 0.5;
        f *= 2.03;
    }
    sum / norm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_deterministic_and_in_range() {
        for i in 0..200 {
            let (x, y) = (i as f32 * 0.37, i as f32 * -0.91);
            let v = fbm(x, y, 3, 7);
            assert!((0.0..=1.0).contains(&v));
            assert_eq!(v, fbm(x, y, 3, 7));
        }
        let mut s = Stream::new(5);
        let mut t = Stream::new(5);
        for _ in 0..50 {
            let a = s.f();
            assert!((0.0..1.0).contains(&a));
            assert_eq!(a, t.f());
        }
    }
}
