use super::ClockError;
pub trait BootClock {
    fn now_ms(&self) -> Result<u64, ClockError>;
}
#[derive(Default, Clone, Copy)]
pub struct SystemBootClock;
impl BootClock for SystemBootClock {
    fn now_ms(&self) -> Result<u64, ClockError> {
        #[cfg(target_os = "linux")]
        {
            let time = rustix::time::clock_gettime_dynamic(rustix::time::DynamicClockId::Boottime)
                .map_err(|_| ClockError::Unavailable)?;
            let seconds = u64::try_from(time.tv_sec).map_err(|_| ClockError::Unavailable)?;
            let nanos = u64::try_from(time.tv_nsec).map_err(|_| ClockError::Unavailable)?;
            seconds
                .checked_mul(1000)
                .and_then(|s| s.checked_add(nanos / 1_000_000))
                .ok_or(ClockError::Unavailable)
        }
        #[cfg(target_os = "windows")]
        {
            let mut ticks = 0_u64;
            // Valid writable u64; this Windows 10+ counter includes sleep/hibernate.
            unsafe {
                windows_sys::Win32::System::WindowsProgramming::QueryInterruptTimePrecise(
                    &mut ticks,
                );
            }
            Ok(ticks / 10_000)
        }
        #[cfg(target_os = "macos")]
        {
            let mut scale = mach2::mach_time::mach_timebase_info_data_t { numer: 0, denom: 0 };
            // Both calls are provided by libSystem. The output pointer is valid.
            if unsafe { mach2::mach_time::mach_timebase_info(&mut scale) } != 0 || scale.denom == 0
            {
                return Err(ClockError::Unavailable);
            }
            let ticks = unsafe { mach2::mach_time::mach_continuous_time() };
            u64::try_from(
                u128::from(ticks) * u128::from(scale.numer) / u128::from(scale.denom) / 1_000_000,
            )
            .map_err(|_| ClockError::Unavailable)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        Err(ClockError::Unavailable)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_suspend_aware_clock_is_available_and_nondecreasing() {
        let before = SystemBootClock.now_ms().unwrap();
        let after = SystemBootClock.now_ms().unwrap();
        assert!(after >= before);
        // Actual suspend/resume requires a dedicated VM/desktop environment.
    }
}
