use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use sysinfo::{Pid, System};

/// Расхождение, начиная с которого считаем сегмент рассинхронизированным. Мельче — это
/// округление длительностей ffmpeg, оно на буфер плеера не влияет.
const DRIFT_TOLERANCE_SECONDS: f64 = 0.5;

/// Счётчик расхождений между картой сегментов и тем, что реально выдал ffmpeg.
///
/// Глобальный, а не поле `MetricsCollector`: пишет его фоновая задача глубоко в транскодере,
/// протаскивать туда состояние приложения ради одного счётчика незачем.
pub static SEGMENT_DRIFT: SegmentDrift = SegmentDrift::new();

pub struct SegmentDrift {
    checked: AtomicU64,
    mismatched: AtomicU64,
    worst_delta_ms: AtomicU64,
}

impl SegmentDrift {
    const fn new() -> Self {
        Self {
            checked: AtomicU64::new(0),
            mismatched: AtomicU64::new(0),
            worst_delta_ms: AtomicU64::new(0),
        }
    }

    /// Учитывает одну проверку. Возвращает величину расхождения, если оно превысило допуск.
    pub fn record(&self, expected: f64, actual: f64) -> Option<f64> {
        self.checked.fetch_add(1, Ordering::Relaxed);
        let delta = (expected - actual).abs();
        if !delta.is_finite() || delta < DRIFT_TOLERANCE_SECONDS {
            return None;
        }
        self.mismatched.fetch_add(1, Ordering::Relaxed);
        self.worst_delta_ms
            .fetch_max((delta * 1000.0) as u64, Ordering::Relaxed);
        Some(delta)
    }

    pub fn snapshot(&self) -> SegmentDriftMetrics {
        SegmentDriftMetrics {
            checked: self.checked.load(Ordering::Relaxed),
            mismatched: self.mismatched.load(Ordering::Relaxed),
            worst_delta_ms: self.worst_delta_ms.load(Ordering::Relaxed),
        }
    }
}

/// Инвариант нарезки: `mismatched > 0` — это баг, даже если жалоб нет.
#[derive(Debug, Clone, Serialize)]
pub struct SegmentDriftMetrics {
    /// Сколько сегментов сверено с картой
    pub checked: u64,
    /// Сколько из них разошлись с картой сильнее допуска
    pub mismatched: u64,
    /// Худшее расхождение в миллисекундах
    pub worst_delta_ms: u64,
}

/// Метрики процесса
#[derive(Debug, Clone, Serialize)]
pub struct ProcessMetrics {
    /// Использование памяти в байтах
    pub memory_bytes: u64,
    /// Использование памяти в мегабайтах
    pub memory_mb: f64,
    /// Использование CPU в процентах
    pub cpu_percent: f64,
    /// Количество открытых файловых дескрипторов
    pub open_files: Option<u64>,
    /// Время работы процесса в секундах
    pub uptime_seconds: u64,
    /// Количество потоков
    pub threads: Option<usize>,
}

/// Системные метрики
#[derive(Debug, Clone, Serialize)]
pub struct SystemMetrics {
    /// Общая память системы в байтах
    pub total_memory_bytes: u64,
    /// Общая память системы в мегабайтах
    pub total_memory_mb: f64,
    /// Используемая память системы в байтах
    pub used_memory_bytes: u64,
    /// Используемая память системы в мегабайтах
    pub used_memory_mb: f64,
    /// Свободная память системы в байтах
    pub free_memory_bytes: u64,
    /// Свободная память системы в мегабайтах
    pub free_memory_mb: f64,
    /// Использование памяти системой в процентах
    pub memory_percent: f64,
    /// Общее количество CPU
    pub cpu_count: usize,
    /// Средняя загрузка CPU в процентах
    pub cpu_usage_percent: f64,
}

/// Полные метрики
#[derive(Debug, Clone, Serialize)]
pub struct Metrics {
    /// Метрики процесса
    pub process: ProcessMetrics,
    /// Системные метрики
    pub system: SystemMetrics,
    /// Расхождения нарезки сегментов
    pub segment_drift: SegmentDriftMetrics,
    /// Время сбора метрик (Unix timestamp)
    pub timestamp: u64,
}

/// Менеджер метрик
pub struct MetricsCollector {
    system: Arc<Mutex<System>>,
    process_id: Pid,
    start_time: Instant,
}

impl MetricsCollector {
    pub fn new() -> Self {
        let mut system = System::new_all();
        system.refresh_all();

        let process_id = Pid::from(std::process::id() as usize);

        Self {
            system: Arc::new(Mutex::new(system)),
            process_id,
            start_time: Instant::now(),
        }
    }

    /// Обновить метрики системы
    pub fn refresh(&self) {
        let mut system = self.system.lock().unwrap();
        system.refresh_all();
        system.refresh_process(self.process_id);
    }

    /// Собрать метрики процесса
    pub fn collect_process_metrics(&self) -> Option<ProcessMetrics> {
        let system = self.system.lock().unwrap();

        if let Some(process) = system.process(self.process_id) {
            let memory_bytes = process.memory();
            let memory_mb = memory_bytes as f64 / 1024.0 / 1024.0;
            let cpu_percent = process.cpu_usage() as f64;
            let uptime_seconds = self.start_time.elapsed().as_secs();

            // Количество открытых файлов недоступно в sysinfo 0.30
            let open_files = None;

            Some(ProcessMetrics {
                memory_bytes,
                memory_mb,
                cpu_percent,
                open_files,
                uptime_seconds,
                threads: None, // sysinfo не предоставляет эту информацию напрямую
            })
        } else {
            None
        }
    }

    /// Собрать системные метрики
    pub fn collect_system_metrics(&self) -> SystemMetrics {
        let system = self.system.lock().unwrap();

        let total_memory_bytes = system.total_memory();
        let total_memory_mb = total_memory_bytes as f64 / 1024.0 / 1024.0;
        let used_memory_bytes = system.used_memory();
        let used_memory_mb = used_memory_bytes as f64 / 1024.0 / 1024.0;
        let free_memory_bytes = system.free_memory();
        let free_memory_mb = free_memory_bytes as f64 / 1024.0 / 1024.0;
        let memory_percent = (used_memory_bytes as f64 / total_memory_bytes as f64) * 100.0;

        let cpu_count = system.cpus().len();
        let cpu_usage_percent = system.global_cpu_info().cpu_usage() as f64;

        SystemMetrics {
            total_memory_bytes,
            total_memory_mb,
            used_memory_bytes,
            used_memory_mb,
            free_memory_bytes,
            free_memory_mb,
            memory_percent,
            cpu_count,
            cpu_usage_percent,
        }
    }

    /// Собрать все метрики
    pub fn collect_all(&self) -> Option<Metrics> {
        self.refresh();

        let process_metrics = self.collect_process_metrics()?;
        let system_metrics = self.collect_system_metrics();

        Some(Metrics {
            process: process_metrics,
            system: system_metrics,
            segment_drift: SEGMENT_DRIFT.snapshot(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        })
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drift_counts_only_deviations_beyond_tolerance() {
        let drift = SegmentDrift::new();

        // Округление длительностей ffmpeg — не рассинхрон.
        assert_eq!(drift.record(6.0, 6.04), None);
        // 2.5 c на сегменте — ровно то, с чего начинался накопленный сдвиг в hls-муксере.
        assert_eq!(drift.record(6.0, 8.5), Some(2.5));
        assert_eq!(drift.record(6.0, f64::NAN), None);

        let snapshot = drift.snapshot();
        assert_eq!(snapshot.checked, 3);
        assert_eq!(snapshot.mismatched, 1);
        assert_eq!(snapshot.worst_delta_ms, 2500);
    }
}
