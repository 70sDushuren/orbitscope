//! 滑动窗口限速器(可同时约束多个窗口,如"每分钟 20 次且每小时 200 次")。

use std::collections::VecDeque;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;

#[derive(Debug)]
pub struct RateLimiter {
    /// (窗口长度, 窗口内最多请求数)
    windows: Vec<(Duration, usize)>,
    history: Mutex<VecDeque<Instant>>,
}

impl RateLimiter {
    /// `windows`:若干 (窗口, 上限)。上限为 0 的窗口被忽略。
    pub fn new(windows: impl IntoIterator<Item = (Duration, usize)>) -> Self {
        Self {
            windows: windows.into_iter().filter(|(_, n)| *n > 0).collect(),
            history: Mutex::new(VecDeque::new()),
        }
    }

    /// 每分钟 / 每小时上限的便捷构造。
    pub fn per_minute_and_hour(per_minute: usize, per_hour: usize) -> Self {
        Self::new([
            (Duration::from_secs(60), per_minute),
            (Duration::from_secs(3_600), per_hour),
        ])
    }

    /// 等待直到允许发出一次请求,并登记该请求。
    pub async fn acquire(&self) {
        loop {
            let wait = {
                let mut h = self.history.lock().await;
                let now = Instant::now();
                let longest = self.windows.iter().map(|(w, _)| *w).max().unwrap_or_default();
                while h.front().is_some_and(|t| now.duration_since(*t) >= longest) {
                    h.pop_front();
                }
                let wait = self
                    .windows
                    .iter()
                    .filter_map(|&(window, limit)| {
                        let in_window: Vec<&Instant> =
                            h.iter().filter(|t| now.duration_since(**t) < window).collect();
                        (in_window.len() >= limit).then(|| {
                            // 最早的那次请求滑出窗口后才可再发
                            let oldest = *in_window[in_window.len() - limit];
                            (oldest + window).saturating_duration_since(now)
                        })
                    })
                    .max()
                    .unwrap_or_default();
                if wait.is_zero() {
                    h.push_back(now);
                    return;
                }
                wait
            };
            tracing::debug!(?wait, "限速等待");
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn enforces_window() {
        let rl = RateLimiter::new([(Duration::from_secs(60), 3)]);
        let start = Instant::now();
        for _ in 0..3 {
            rl.acquire().await;
        }
        assert!(start.elapsed() < Duration::from_millis(1));
        rl.acquire().await; // 第 4 次需等到第 1 次滑出窗口
        assert!(start.elapsed() >= Duration::from_secs(60));
        assert!(start.elapsed() < Duration::from_secs(61));
    }

    #[tokio::test(start_paused = true)]
    async fn multiple_windows() {
        let rl = RateLimiter::new([(Duration::from_secs(1), 10), (Duration::from_secs(10), 2)]);
        let start = Instant::now();
        rl.acquire().await;
        rl.acquire().await;
        rl.acquire().await; // 受 10 s / 2 次约束
        assert!(start.elapsed() >= Duration::from_secs(10));
    }
}
