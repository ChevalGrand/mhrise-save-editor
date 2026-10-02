use num_integer::Integer as IntegerOps;

pub mod backend {
  pub mod num_bigint {
    pub use num_bigint::{BigInt as Integer, Sign};

    pub fn bytes_to_int(bytes: &[u8]) -> Integer {
      Integer::from_bytes_le(Sign::Plus, bytes)
    }

    pub fn int_to_bytes_le<const N: usize>(value: &Integer) -> [u8; N] {
      let mut output = [0u8; N];
      let bytes = value.to_bytes_le().1;
      let length = bytes.len().min(N);
      output[..length].copy_from_slice(&bytes[..length]);
      output
    }
  }
}

pub trait EccInteger: Sized + Clone + PartialOrd {
  fn from_u64(value: u64) -> Self;
  fn mod_exp(&self, exponent: &Self, modulus: &Self) -> Self;
  fn from_bytes_le(bytes: &[u8]) -> Self;
  fn to_bytes_le<const N: usize>(&self) -> [u8; N];
  fn add_mod(&self, other: &Self, modulus: &Self) -> Self;
  fn sub_mod(&self, other: &Self, modulus: &Self) -> Self;
  fn mul_mod(&self, other: &Self, modulus: &Self) -> Self;
  fn is_odd(&self) -> bool;
  fn div_2(&self) -> Self;

  fn to_u64_le_bytes(&self) -> [u8; 8] {
    let bytes = self.to_bytes_le::<32>();
    bytes[..8].try_into().expect("fixed-size slice")
  }
}

impl EccInteger for num_bigint::BigInt {
  fn from_u64(value: u64) -> Self {
    Self::from(value)
  }

  fn mod_exp(&self, exponent: &Self, modulus: &Self) -> Self {
    self.modpow(exponent, modulus)
  }

  fn from_bytes_le(bytes: &[u8]) -> Self {
    Self::from_bytes_le(num_bigint::Sign::Plus, bytes)
  }

  fn to_bytes_le<const N: usize>(&self) -> [u8; N] {
    backend::num_bigint::int_to_bytes_le(self)
  }

  fn add_mod(&self, other: &Self, modulus: &Self) -> Self {
    (self + other).mod_floor(modulus)
  }

  fn sub_mod(&self, other: &Self, modulus: &Self) -> Self {
    (self - other).mod_floor(modulus)
  }

  fn mul_mod(&self, other: &Self, modulus: &Self) -> Self {
    (self * other).mod_floor(modulus)
  }

  fn is_odd(&self) -> bool {
    !self.is_even()
  }

  fn div_2(&self) -> Self {
    self >> 1
  }
}

fn mod_inverse<T: EccInteger>(value: &T, modulus: &T) -> T {
  let two = T::from_u64(2);
  value.mod_exp(&modulus.sub_mod(&two, modulus), modulus)
}

pub fn point_add<T: EccInteger>(
  first: Option<(T, T)>,
  second: Option<(T, T)>,
  a: &T,
  modulus: &T,
) -> Option<(T, T)> {
  let Some((x1, y1)) = first else {
    return second;
  };
  let Some((x2, y2)) = second else {
    return Some((x1, y1));
  };

  if x1 == x2 && (y1 != y2 || y1 == T::from_u64(0)) {
    return None;
  }

  let slope = if x1 == x2 && y1 == y2 {
    let numerator = x1.mul_mod(&x1, modulus).mul_mod(&T::from_u64(3), modulus).add_mod(a, modulus);
    let denominator = y1.mul_mod(&T::from_u64(2), modulus);
    numerator.mul_mod(&mod_inverse(&denominator, modulus), modulus)
  } else {
    let numerator = y2.sub_mod(&y1, modulus);
    let denominator = x2.sub_mod(&x1, modulus);
    numerator.mul_mod(&mod_inverse(&denominator, modulus), modulus)
  };

  let x3 = slope.mul_mod(&slope, modulus).sub_mod(&x1, modulus).sub_mod(&x2, modulus);
  let y3 = slope.mul_mod(&x1.sub_mod(&x3, modulus), modulus).sub_mod(&y1, modulus);
  Some((x3, y3))
}

pub fn scalar_mult<T: EccInteger>(k: &T, point: (T, T), a: &T, modulus: &T) -> Option<(T, T)> {
  let mut result = None;
  let mut addend = Some(point);
  let mut factor = k.clone();

  while factor > T::from_u64(0) {
    if factor.is_odd() {
      result = point_add(result, addend.clone(), a, modulus);
    }
    if let Some(value) = addend {
      addend = point_add(Some(value.clone()), Some(value), a, modulus);
    }
    factor = factor.div_2();
  }
  result
}
