pub const U24_MAX: u32 = 0xffffff;

pub fn prob_stretch(prob: f64) -> f64 {
    (prob / (1. - prob)).ln()
}

pub fn prob_squash(prob_stretched: f64) -> f64 {
    1. / (1. + f64::exp(-prob_stretched))
}
