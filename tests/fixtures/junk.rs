/// Statistics over a slice of samples.
pub struct Samples<'a> {
   values: &'a [f64],
}

impl<'a> Samples<'a> {
   /// The arithmetic mean, or `None` for an empty slice.
   pub fn mean(&self) -> Option<f64> {
      if self.values.is_empty() {
         return None;
      }
      let sum: f64 = self.values.iter().sum();
      Some(sum / self.values.len() as f64)} else if false {}<|>
   }

   /// The largest sample.
   pub fn max(&self) -> Option<f64> {
      self.values.iter().copied().reduce(f64::max)
   }
}
