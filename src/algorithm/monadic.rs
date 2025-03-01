//! Algorithms for monadic array operations

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap},
    iter::{once, repeat},
    ptr,
};

use ecow::EcoVec;
use rayon::prelude::*;
use tinyvec::tiny_vec;

use crate::{
    array::*,
    cowslice::{cowslice, CowSlice},
    value::Value,
    Uiua, UiuaResult,
};

use super::{ArrayCmpSlice, FillContext};

impl Value {
    /// Make the value 1-dimensional
    pub fn deshape(&mut self) {
        self.generic_mut_deep(
            Array::deshape,
            Array::deshape,
            Array::deshape,
            Array::deshape,
        )
    }
    /// Attempt to parse the value into a number
    pub fn parse_num(&self, env: &Uiua) -> UiuaResult<Self> {
        Ok(self
            .as_string(env, "Parsed array must be a string")?
            .parse::<f64>()
            .map_err(|e| env.error(format!("Cannot parse into number: {}", e)))?
            .into())
    }
}

impl<T: ArrayValue> Array<T> {
    /// Make the array 1-dimensional
    pub fn deshape(&mut self) {
        self.shape = tiny_vec![self.element_count()];
    }
}

impl Value {
    /// Create a `range` array
    pub fn range(&self, env: &Uiua) -> UiuaResult<Self> {
        let shape = &self.as_nats(
            env,
            "Range max should be a single natural number \
            or a list of natural numbers",
        )?;
        let mut shape = Shape::from(shape.as_slice());
        let data = range(&shape, env)?;
        if shape.len() > 1 {
            shape.push(shape.len());
        }
        Ok(Array::new(shape, data).into())
    }
}

fn range(shape: &[usize], env: &Uiua) -> UiuaResult<CowSlice<f64>> {
    if shape.is_empty() {
        return Ok(cowslice![0.0]);
    }
    if shape.contains(&0) {
        return Ok(CowSlice::new());
    }
    let mut len = shape.len();
    for &item in shape {
        let (new, overflow) = len.overflowing_mul(item);
        if overflow {
            let len = shape.len() as f64 * shape.iter().map(|d| *d as f64).product::<f64>();
            return Err(env.error(format!(
                "Attempting to make a range from shape {} would \
                create an array with {} elements, which is too large",
                FormatShape(shape),
                len
            )));
        }
        len = new;
    }
    let mut data: EcoVec<f64> = EcoVec::with_capacity(len);
    let mut curr = vec![0; shape.len()];
    loop {
        for d in &curr {
            data.push(*d as f64);
        }
        let mut i = shape.len() - 1;
        loop {
            curr[i] += 1;
            if curr[i] == shape[i] {
                curr[i] = 0;
                if i == 0 {
                    return Ok(data.into());
                }
                i -= 1;
            } else {
                break;
            }
        }
    }
}

impl Value {
    /// Get the first row of the value
    pub fn first(self, env: &Uiua) -> UiuaResult<Self> {
        self.generic_into_deep(
            |a| a.first(env).map(Into::into),
            |a| a.first(env).map(Into::into),
            |a| a.first(env).map(Into::into),
            |a| a.first(env).map(Into::into),
        )
    }
    /// Get the last row of the value
    pub fn last(self, env: &Uiua) -> UiuaResult<Self> {
        self.generic_into_deep(
            |a| a.last(env).map(Into::into),
            |a| a.last(env).map(Into::into),
            |a| a.last(env).map(Into::into),
            |a| a.last(env).map(Into::into),
        )
    }
}

impl<T: ArrayValue> Array<T> {
    /// Get the first row of the array
    pub fn first(mut self, env: &Uiua) -> UiuaResult<Self> {
        match &*self.shape {
            [] => Err(env.error("Cannot take first of a scalar")),
            [0, rest @ ..] => {
                if let Some(fill) = env.fill() {
                    self.data.extend(repeat(fill).take(self.row_len()));
                    self.shape = rest.into();
                    Ok(self)
                } else {
                    Err(env.error("Cannot take first of an empty array").fill())
                }
            }
            _ => {
                let row_len = self.row_len();
                self.shape.remove(0);
                self.data.truncate(row_len);
                Ok(self)
            }
        }
    }
    /// Get the last row of the array
    pub fn last(mut self, env: &Uiua) -> UiuaResult<Self> {
        match &*self.shape {
            [] => Err(env.error("Cannot take last of a scalar")),
            [0, rest @ ..] => {
                if let Some(fill) = env.fill() {
                    self.data.extend(repeat(fill).take(self.row_len()));
                    self.shape = rest.into();
                    Ok(self)
                } else {
                    Err(env.error("Cannot take last of an empty array").fill())
                }
            }
            _ => {
                let row_len = self.row_len();
                self.shape.remove(0);
                let prefix_len = self.data.len() - row_len;
                self.data = self.data.into_iter().skip(prefix_len).collect();
                Ok(self)
            }
        }
    }
}

impl Value {
    /// Reverse the rows of the value
    pub fn reverse(&mut self) {
        self.generic_mut_deep(
            Array::reverse,
            Array::reverse,
            Array::reverse,
            Array::reverse,
        )
    }
}

impl<T: ArrayValue> Array<T> {
    /// Reverse the rows of the array
    pub fn reverse(&mut self) {
        if self.shape.is_empty() || self.element_count() == 0 {
            return;
        }
        let row_count = self.row_count();
        let row_len = self.row_len();
        let data = self.data.as_mut_slice();
        for i in 0..row_count / 2 {
            let left = i * row_len;
            let right = (row_count - i - 1) * row_len;
            let left = &mut data[left] as *mut T;
            let right = &mut data[right] as *mut T;
            unsafe {
                ptr::swap_nonoverlapping(left, right, row_len);
            }
        }
    }
}

impl Value {
    /// Transpose the value
    pub fn transpose(&mut self) {
        self.generic_mut_deep(
            Array::transpose,
            Array::transpose,
            Array::transpose,
            Array::transpose,
        )
    }
    /// Inverse transpose the value
    pub fn inv_transpose(&mut self) {
        self.generic_mut_deep(
            Array::inv_transpose,
            Array::inv_transpose,
            Array::inv_transpose,
            Array::inv_transpose,
        )
    }
}

impl<T: ArrayValue> Array<T> {
    /// Transpose the array
    pub fn transpose(&mut self) {
        crate::profile_function!();
        if self.shape.len() < 2 {
            return;
        }
        if self.shape[0] == 0 {
            self.shape.rotate_left(1);
            return;
        }
        let mut temp = EcoVec::with_capacity(self.data.len());
        let row_len = self.row_len();
        let row_count = self.row_count();
        for j in 0..row_len {
            for i in 0..row_count {
                temp.push(self.data[i * row_len + j].clone());
            }
        }
        self.data = temp.into();
        self.shape.rotate_left(1);
    }
    /// Inverse transpose the array
    pub fn inv_transpose(&mut self) {
        crate::profile_function!();
        if self.shape.len() < 2 {
            return;
        }
        if self.shape[0] == 0 {
            self.shape.rotate_right(1);
            return;
        }
        let mut temp = EcoVec::with_capacity(self.data.len());
        let col_len = *self.shape.last().unwrap();
        let col_count: usize = self.shape.iter().rev().skip(1).product();
        for j in 0..col_len {
            for i in 0..col_count {
                temp.push(self.data[i * col_len + j].clone());
            }
        }
        self.data = temp.into();
        self.shape.rotate_right(1);
    }
}

impl Value {
    /// Get the `rise` of the value
    pub fn rise(&self, env: &Uiua) -> UiuaResult<Vec<usize>> {
        self.generic_ref_env_deep(Array::rise, Array::rise, Array::rise, Array::rise, env)
    }
    /// Get the `fall` of the value
    pub fn fall(&self, env: &Uiua) -> UiuaResult<Vec<usize>> {
        self.generic_ref_env_deep(Array::fall, Array::fall, Array::fall, Array::fall, env)
    }
    /// `classify` the rows of the value
    pub fn classify(&self, env: &Uiua) -> UiuaResult<Self> {
        self.generic_ref_env_deep(
            Array::classify,
            Array::classify,
            Array::classify,
            Array::classify,
            env,
        )
        .map(Self::from_iter)
    }
    /// `deduplicate` the rows of the value
    pub fn deduplicate(&mut self) {
        self.generic_mut_deep(
            Array::deduplicate,
            Array::deduplicate,
            Array::deduplicate,
            Array::deduplicate,
        )
    }
}

impl<T: ArrayValue> Array<T> {
    /// Get the `rise` of the array
    pub fn rise(&self, env: &Uiua) -> UiuaResult<Vec<usize>> {
        if self.rank() == 0 {
            return Err(env.error("Cannot rise a scalar"));
        }
        if self.element_count() == 0 {
            return Ok(Vec::new());
        }
        let mut indices = (0..self.row_count()).collect::<Vec<_>>();
        indices.par_sort_by(|&a, &b| {
            self.row_slice(a)
                .iter()
                .zip(self.row_slice(b))
                .map(|(a, b)| a.array_cmp(b))
                .find(|x| x != &Ordering::Equal)
                .unwrap_or(Ordering::Equal)
        });
        Ok(indices)
    }
    /// Get the `fall` of the array
    pub fn fall(&self, env: &Uiua) -> UiuaResult<Vec<usize>> {
        if self.rank() == 0 {
            return Err(env.error("Cannot fall a scalar"));
        }
        if self.element_count() == 0 {
            return Ok(Vec::new());
        }
        let mut indices = (0..self.row_count()).collect::<Vec<_>>();
        indices.par_sort_by(|&a, &b| {
            self.row_slice(a)
                .iter()
                .zip(self.row_slice(b))
                .map(|(a, b)| b.array_cmp(a))
                .find(|x| x != &Ordering::Equal)
                .unwrap_or(Ordering::Equal)
        });
        Ok(indices)
    }
    /// `classify` the rows of the array
    pub fn classify(&self, env: &Uiua) -> UiuaResult<Vec<usize>> {
        if self.rank() == 0 {
            return Err(env.error("Cannot classify a rank-0 array"));
        }
        let mut classes = BTreeMap::new();
        let mut classified = Vec::with_capacity(self.row_count());
        for row in self.rows() {
            let new_class = classes.len();
            let class = *classes.entry(row).or_insert(new_class);
            classified.push(class);
        }
        Ok(classified)
    }
    /// `deduplicate` the rows of the array
    pub fn deduplicate(&mut self) {
        if self.rank() == 0 {
            return;
        }
        let mut deduped = CowSlice::new();
        let mut seen = BTreeSet::new();
        let mut new_len = 0;
        for row in self.rows() {
            if seen.insert(row.clone()) {
                deduped.extend_from_slice(&row.data);
                new_len += 1;
            }
        }
        self.data = deduped;
        self.shape[0] = new_len;
    }
}

impl Value {
    /// Encode the `bits` of the value
    pub fn bits(&self, env: &Uiua) -> UiuaResult<Array<u8>> {
        match self {
            Value::Byte(n) => n.convert_ref().bits(env),
            Value::Num(n) => n.bits(env),
            _ => Err(env.error("Argument to bits must be an array of natural numbers")),
        }
    }
    /// Decode the `bits` of the value
    pub fn inverse_bits(&self, env: &Uiua) -> UiuaResult<Array<f64>> {
        match self {
            Value::Byte(n) => n.inverse_bits(env),
            Value::Num(n) => n.convert_ref_with(|n| n as u8).inverse_bits(env),
            _ => Err(env.error("Argument to inverse_bits must be an array of naturals")),
        }
    }
}

impl Array<f64> {
    /// Encode the `bits` of the array
    pub fn bits(&self, env: &Uiua) -> UiuaResult<Array<u8>> {
        let mut nats = Vec::new();
        for &n in &self.data {
            if n.fract() != 0.0 {
                return Err(env.error("Array must be a list of naturals"));
            }
            nats.push(n as u128);
        }
        let mut max = if let Some(max) = nats.iter().max() {
            *max
        } else {
            let mut shape = self.shape.clone();
            shape.push(0);
            return Ok(Array::new(shape, CowSlice::new()));
        };
        let mut max_bits = 0;
        while max != 0 {
            max_bits += 1;
            max >>= 1;
        }
        let mut new_data = EcoVec::with_capacity(self.data.len() * max_bits);
        // Little endian
        for n in nats {
            for i in 0..max_bits {
                new_data.push(u8::from(n & (1 << i) != 0));
            }
        }
        let mut shape = self.shape.clone();
        shape.push(max_bits);
        let arr = Array::new(shape, new_data);
        arr.validate_shape();
        Ok(arr)
    }
}

impl Array<u8> {
    /// Decode the `bits` of the array
    pub fn inverse_bits(&self, env: &Uiua) -> UiuaResult<Array<f64>> {
        let mut bools = Vec::with_capacity(self.data.len());
        for &b in &self.data {
            if b > 1 {
                return Err(env.error("Array must be a list of booleans"));
            }
            bools.push(b != 0);
        }
        if bools.is_empty() {
            if self.shape.is_empty() {
                return Ok(Array::from(0.0));
            }
            let mut shape = self.shape.clone();
            shape[0] = 0;
            return Ok(Array::new(shape, CowSlice::new()));
        }
        if self.rank() == 0 {
            return Ok(Array::from(bools[0] as u8 as f64));
        }
        let mut shape = self.shape.clone();
        let bit_string_len = shape.pop().unwrap();
        let mut new_data = EcoVec::with_capacity(self.data.len() / bit_string_len);
        // Little endian
        for bits in bools.chunks_exact(bit_string_len) {
            let mut n: u128 = 0;
            for (i, b) in bits.iter().enumerate() {
                if *b {
                    n |= 1u128.overflowing_shl(i as u32).0;
                }
            }
            new_data.push(n as f64);
        }
        let arr = Array::new(shape, new_data);
        arr.validate_shape();
        Ok(arr)
    }
}

impl Value {
    /// Get the indices `where` the value is nonzero
    pub fn wher(&self, env: &Uiua) -> UiuaResult<Array<f64>> {
        let counts = self.as_nats(env, "Argument to where must be a list of naturals")?;
        let total: usize = counts.iter().fold(0, |acc, &b| acc.saturating_add(b));
        let mut data = EcoVec::with_capacity(total);
        for (i, &b) in counts.iter().enumerate() {
            for _ in 0..b {
                let i = i as f64;
                data.push(i);
            }
        }
        Ok(Array::from(data))
    }
    /// Get the `first` index `where` the value is nonzero
    pub fn first_where(&self, env: &Uiua) -> UiuaResult<f64> {
        if self.rank() > 1 {
            return Err(env.error(format!(
                "Argument to where must be a list of naturals, but it is rank {}",
                self.rank()
            )));
        }
        match self {
            Value::Num(nums) => {
                for (i, n) in nums.data.iter().enumerate() {
                    if n.fract() != 0.0 || *n < 0.0 {
                        return Err(env.error("Argument to where must be a list of naturals"));
                    }
                    if *n != 0.0 {
                        return Ok(i as f64);
                    }
                }
                env.fill::<f64>()
                    .ok_or_else(|| env.error("Cannot take first of an empty array"))
            }
            Value::Byte(bytes) => {
                for (i, n) in bytes.data.iter().enumerate() {
                    if *n != 0 {
                        return Ok(i as f64);
                    }
                }
                env.fill::<f64>()
                    .ok_or_else(|| env.error("Cannot take first of an empty array"))
            }
            value => Err(env.error(format!(
                "Argument to where must be a list of naturals, but it is {}",
                value.type_name_plural()
            ))),
        }
    }
    /// `invert` `where`
    pub fn inverse_where(&self, env: &Uiua) -> UiuaResult<Self> {
        let indices = self.as_nats(env, "Argument to inverse where must be a list of naturals")?;
        let is_sorted = indices
            .iter()
            .zip(indices.iter().skip(1))
            .all(|(&a, &b)| a <= b);
        let size = indices.iter().max().map(|&i| i + 1).unwrap_or(0);
        let mut data = EcoVec::with_capacity(size);
        if is_sorted {
            let mut j = 0;
            for i in 0..size {
                while indices.get(j).is_some_and(|&n| n < i) {
                    j += 1;
                }
                let mut count: usize = 0;
                while indices.get(j).copied() == Some(i) {
                    j += 1;
                    count += 1;
                }
                data.push(count as f64);
            }
        } else {
            let mut counts = HashMap::new();
            for &i in &indices {
                *counts.entry(i).or_insert(0) += 1;
            }
            for i in 0..size {
                let count = counts.get(&i).copied().unwrap_or(0);
                data.push(count as f64);
            }
        }
        Ok(Array::from(data).into())
    }
}

impl Value {
    /// Convert a string value to a list of UTF-8 bytes
    pub fn utf8(&self, env: &Uiua) -> UiuaResult<Self> {
        let s = self.as_string(env, "Argument to utf must be a string")?;
        Ok(Array::<u8>::from_iter(s.into_bytes()).into())
    }
    /// Convert a list of UTF-8 bytes to a string value
    pub fn inv_utf8(&self, env: &Uiua) -> UiuaResult<Self> {
        let bytes = self.as_bytes(env, "Argument to inverse utf must be a list of bytes")?;
        let s = String::from_utf8(bytes).map_err(|e| env.error(e))?;
        Ok(s.into())
    }
}

impl Value {
    /// Join an ocean value
    pub fn ocean(mut self, val: f64, env: &Uiua) -> UiuaResult<Self> {
        match &mut self {
            Value::Num(n) => n.ocean(val),
            Value::Byte(b) => {
                if val.fract() == 0.0 && (0.0..=255.0).contains(&val) {
                    b.ocean(val as u8);
                } else {
                    let mut arr = b.convert_ref();
                    arr.ocean(val);
                    self = arr.into();
                }
            }
            val => {
                return Err(env.error(format!(
                    "Cannot join ocean values to {} array",
                    val.type_name()
                )))
            }
        }
        Ok(self)
    }
}

impl<T: ArrayValue> Array<T> {
    /// Join an ocean value
    pub fn ocean(&mut self, value: T) {
        if self.rank() == 0 {
            self.data.extend(once(value));
            self.data.as_mut_slice().rotate_right(1);
            self.shape = tiny_vec![2];
        } else {
            let row_len = self.row_len();
            self.data.extend(repeat(value).take(row_len));
            self.data.as_mut_slice().rotate_right(row_len);
            self.shape[0] += 1;
        }
    }
}

impl Value {
    pub(crate) fn first_min_index(&self, env: &Uiua) -> UiuaResult<Self> {
        self.generic_ref_env_deep(
            Array::first_min_index,
            Array::first_min_index,
            Array::first_min_index,
            Array::first_min_index,
            env,
        )
        .map(Into::into)
    }
    pub(crate) fn first_max_index(&self, env: &Uiua) -> UiuaResult<Self> {
        self.generic_ref_env_deep(
            Array::first_max_index,
            Array::first_max_index,
            Array::first_max_index,
            Array::first_max_index,
            env,
        )
        .map(Into::into)
    }
    pub(crate) fn last_min_index(&self, env: &Uiua) -> UiuaResult<Self> {
        self.generic_ref_env_deep(
            Array::last_min_index,
            Array::last_min_index,
            Array::last_min_index,
            Array::last_min_index,
            env,
        )
        .map(Into::into)
    }
    pub(crate) fn last_max_index(&self, env: &Uiua) -> UiuaResult<Self> {
        self.generic_ref_env_deep(
            Array::last_max_index,
            Array::last_max_index,
            Array::last_max_index,
            Array::last_max_index,
            env,
        )
        .map(Into::into)
    }
}

impl<T: ArrayValue> Array<T> {
    pub(crate) fn first_min_index(&self, env: &Uiua) -> UiuaResult<f64> {
        if self.rank() == 0 {
            return Err(env.error("Cannot get min index of a scalar"));
        }
        if self.row_count() == 0 {
            return env
                .fill::<f64>()
                .ok_or_else(|| env.error("Cannot get min index of an empty array"));
        }
        let index = self
            .row_slices()
            .map(ArrayCmpSlice)
            .enumerate()
            .min_by(|(_, a), (_, b)| a.cmp(b))
            .unwrap()
            .0;
        Ok(index as f64)
    }
    pub(crate) fn first_max_index(&self, env: &Uiua) -> UiuaResult<f64> {
        if self.rank() == 0 {
            return Err(env.error("Cannot get max index of a scalar"));
        }
        if self.row_count() == 0 {
            return env
                .fill::<f64>()
                .ok_or_else(|| env.error("Cannot get max index of an empty array"));
        }
        let index = self
            .row_slices()
            .map(ArrayCmpSlice)
            .enumerate()
            .min_by(|(_, a), (_, b)| a.cmp(b).reverse())
            .unwrap()
            .0;
        Ok(index as f64)
    }
    pub(crate) fn last_min_index(&self, env: &Uiua) -> UiuaResult<f64> {
        if self.rank() == 0 {
            return Err(env.error("Cannot get min index of a scalar"));
        }
        if self.row_count() == 0 {
            return env
                .fill::<f64>()
                .ok_or_else(|| env.error("Cannot get min index of an empty array"));
        }
        let index = self
            .row_slices()
            .map(ArrayCmpSlice)
            .enumerate()
            .max_by(|(_, a), (_, b)| a.cmp(b).reverse())
            .unwrap()
            .0;
        Ok(index as f64)
    }
    pub(crate) fn last_max_index(&self, env: &Uiua) -> UiuaResult<f64> {
        if self.rank() == 0 {
            return Err(env.error("Cannot get max index of a scalar"));
        }
        if self.row_count() == 0 {
            return env
                .fill::<f64>()
                .ok_or_else(|| env.error("Cannot get max index of an empty array"));
        }
        let index = self
            .row_slices()
            .map(ArrayCmpSlice)
            .enumerate()
            .max_by(|(_, a), (_, b)| a.cmp(b))
            .unwrap()
            .0;
        Ok(index as f64)
    }
}
