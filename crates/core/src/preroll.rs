use std::collections::VecDeque;

/// 直近 N サンプルだけを保持する固定長リングバッファ。
pub struct PreRoll {
    capacity: usize,
    buf: VecDeque<i16>,
}

impl PreRoll {
    pub fn new(capacity_samples: usize) -> Self {
        Self {
            capacity: capacity_samples,
            buf: VecDeque::with_capacity(capacity_samples),
        }
    }

    /// 追記する。容量を超えた分は古い方から捨てる。
    pub fn push(&mut self, samples: &[i16]) {
        self.buf.extend(samples.iter().copied());
        while self.buf.len() > self.capacity {
            let _ = self.buf.pop_front();
        }
    }

    /// 中身を全部取り出して空にする。
    pub fn take(&mut self) -> Vec<i16> {
        self.buf.drain(..).collect()
    }

    pub fn clear(&mut self) {
        self.buf.clear();
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::PreRoll;

    #[test]
    fn keeps_only_capacity() {
        let mut preroll = PreRoll::new(3);
        preroll.push(&[1, 2]);
        preroll.push(&[3, 4]);
        assert_eq!(preroll.len(), 3);
        assert_eq!(preroll.take(), vec![2, 3, 4]);
    }

    #[test]
    fn take_empties() {
        let mut preroll = PreRoll::new(3);
        preroll.push(&[1, 2]);
        let _ = preroll.take();
        assert_eq!(preroll.len(), 0);
    }

    #[test]
    fn take_preserves_order() {
        let mut preroll = PreRoll::new(5);
        preroll.push(&[1, 2, 3, 4]);
        assert_eq!(preroll.take(), vec![1, 2, 3, 4]);
    }
}
