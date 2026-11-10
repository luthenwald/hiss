/// Statistics over a slice of samples.
pub struct Samples<'a> {
   values: &'a [f64],
}

impl<'a> Samples<'a> {
   pub fn new(values: &'a [f64]) -> Self {
      Samples { values }
   }

   /// The arithmetic mean, or `None` for an empty slice.
   pub fn mean(&self) -> Option<f64> {
      if self.values.is_empty() {
         return None;
      }
      let sum: f64 = self.values.it<|>
   }
}
