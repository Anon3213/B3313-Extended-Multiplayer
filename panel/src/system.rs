//! What the panel asks Windows about the user's settings: the time of day, how they like times
//! written, and whether they turned on a contrast theme. Elsewhere it falls back to plain
//! defaults.

use std::time::Duration;

/// Seconds in a day.
const DAY: u64 = 24 * 60 * 60;

/// The local time of day, as time since midnight.
#[must_use]
pub fn local_time_of_day() -> Duration {
    #[cfg(windows)]
    {
        windows_time_of_day()
    }
    #[cfg(not(windows))]
    {
        // Without the time zone, UTC.
        let since_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        Duration::from_secs(since_epoch.as_secs() % DAY)
            + Duration::from_nanos(u64::from(since_epoch.subsec_nanos()))
    }
}

/// Wraps a time of day past midnight back into one day.
#[must_use]
pub fn wrap_to_day(time: Duration) -> Duration {
    Duration::from_secs(time.as_secs() % DAY) + Duration::from_nanos(u64::from(time.subsec_nanos()))
}

#[cfg(windows)]
fn windows_time_of_day() -> Duration {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    // SAFETY: GetLocalTime only fills in the SYSTEMTIME it returns; it has no preconditions.
    let now = unsafe { GetLocalTime() };
    Duration::from_secs(
        u64::from(now.wHour) * 3600 + u64::from(now.wMinute) * 60 + u64::from(now.wSecond),
    ) + Duration::from_millis(u64::from(now.wMilliseconds))
}

/// Whether the user's region writes times with a 24-hour clock, as the panel's default.
#[must_use]
pub fn prefers_24_hour() -> bool {
    #[cfg(windows)]
    {
        windows_prefers_24_hour().unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// Reads the user's time format, such as "h:mm:ss tt" or "HH:mm:ss": a capital H is the
/// 24-hour clock.
#[cfg(windows)]
fn windows_prefers_24_hour() -> Option<bool> {
    use windows::Win32::Globalization::{GetLocaleInfoEx, LOCALE_STIMEFORMAT};
    use windows::core::PCWSTR;
    let mut format = [0_u16; 80];
    // SAFETY: a null locale name means the user's own; the buffer is a valid, writable slice,
    // whose length the wrapper passes, so Windows writes within it.
    let written = unsafe { GetLocaleInfoEx(PCWSTR::null(), LOCALE_STIMEFORMAT, Some(&mut format)) };
    let length = usize::try_from(written).ok()?.checked_sub(1)?;
    let text = String::from_utf16_lossy(format.get(..length)?);
    Some(text.contains('H'))
}

/// Whether one of Windows' contrast themes is on, which the panel then follows.
#[must_use]
pub fn high_contrast_on() -> bool {
    #[cfg(windows)]
    {
        windows_high_contrast_on()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
fn windows_high_contrast_on() -> bool {
    use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let Ok(size) = u32::try_from(size_of::<HIGHCONTRASTW>()) else {
        return false;
    };
    let mut contrast = HIGHCONTRASTW {
        cbSize: size,
        ..HIGHCONTRASTW::default()
    };
    // SAFETY: SPI_GETHIGHCONTRAST writes a HIGHCONTRASTW, whose size `cbSize` gives, to the
    // pointer passed; `contrast` is one and outlives the call. It changes no setting.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            size,
            Some((&raw mut contrast).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    read.is_ok() && contrast.dwFlags.contains(HCF_HIGHCONTRASTON)
}

/// Whether Windows' "Animation effects" (Settings, Accessibility, Visual effects) is on. The
/// panel's own motion follows it: off, everything changes at once.
#[must_use]
pub fn animations_on() -> bool {
    #[cfg(windows)]
    {
        windows_animations_on()
    }
    #[cfg(not(windows))]
    {
        true
    }
}

#[cfg(windows)]
fn windows_animations_on() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    use windows::core::BOOL;
    let mut on = BOOL(1);
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL to the pointer passed; `on` is one and
    // outlives the call. It changes no setting.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&raw mut on).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    // Animate if the setting cannot be read, as Windows does by default.
    read.is_err() || on.as_bool()
}

/// A date and time of day, to the second.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalTime {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
}

/// The local date and time; elsewhere than Windows, UTC.
#[must_use]
pub fn local_now() -> LocalTime {
    #[cfg(windows)]
    {
        use windows::Win32::System::SystemInformation::GetLocalTime;
        // SAFETY: GetLocalTime only fills in the SYSTEMTIME it returns; it has no preconditions.
        let now = unsafe { GetLocalTime() };
        LocalTime {
            year: now.wYear,
            month: now.wMonth,
            day: now.wDay,
            hour: now.wHour,
            minute: now.wMinute,
            second: now.wSecond,
        }
    }
    #[cfg(not(windows))]
    {
        let since_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        utc_from_unix(since_epoch.as_secs())
    }
}

/// The UTC date and time `seconds` after 1970 began, by Howard Hinnant's days-to-civil method.
#[cfg_attr(
    all(windows, not(test)),
    expect(dead_code, reason = "Windows gives the local time itself")
)]
fn utc_from_unix(seconds: u64) -> LocalTime {
    let (days, rest) = (seconds / DAY, seconds % DAY);
    // Days since 1 March of year 0, in 400-year eras, so leap days fall at each year's end.
    let shifted = days + 719_468;
    let (era, day_of_era) = (shifted / 146_097, shifted % 146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    let year = era * 400 + year_of_era + u64::from(month <= 2);
    let part = |value: u64| u16::try_from(value).unwrap_or(u16::MAX);
    LocalTime {
        year: part(year),
        month: part(month),
        day: part(day_of_year - (153 * month_from_march + 2) / 5 + 1),
        hour: part(rest / 3600),
        minute: part(rest / 60 % 60),
        second: part(rest % 60),
    }
}

/// Memory, in bytes: what the panel uses, and what the PC has free and in all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Memory {
    pub panel: u64,
    pub available: u64,
    pub total: u64,
}

/// The panel's and the PC's memory, where Windows tells it.
#[must_use]
pub fn memory() -> Option<Memory> {
    #[cfg(windows)]
    {
        windows_memory()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(windows)]
fn windows_memory() -> Option<Memory> {
    use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: u32::try_from(size_of::<PROCESS_MEMORY_COUNTERS>()).ok()?,
        ..PROCESS_MEMORY_COUNTERS::default()
    };
    // SAFETY: GetCurrentProcess returns this process's pseudo-handle; it has no preconditions.
    let process = unsafe { GetCurrentProcess() };
    // SAFETY: `process` is valid for the process's life; `counters` is a writable
    // PROCESS_MEMORY_COUNTERS of the size given in `cb`, and outlives the call.
    let read = unsafe { K32GetProcessMemoryInfo(process, &raw mut counters, counters.cb) };
    if !read.as_bool() {
        return None;
    }
    let mut status = MEMORYSTATUSEX {
        dwLength: u32::try_from(size_of::<MEMORYSTATUSEX>()).ok()?,
        ..MEMORYSTATUSEX::default()
    };
    // SAFETY: `status` is a writable MEMORYSTATUSEX whose `dwLength` gives its size, as the call
    // requires, and outlives it.
    unsafe { GlobalMemoryStatusEx(&raw mut status) }.ok()?;
    Some(Memory {
        panel: u64::try_from(counters.WorkingSetSize).ok()?,
        available: status.ullAvailPhys,
        total: status.ullTotalPhys,
    })
}

/// Windows' release and build, as `Windows 11 24H2, build 26100.4351`.
#[must_use]
pub fn windows_version() -> Option<String> {
    #[cfg(windows)]
    {
        // Read from the registry: the version functions report Windows 8 to a program whose
        // manifest does not name later versions, as GPUI's does not.
        let build: u32 = windows_current_version("CurrentBuildNumber")?
            .trim()
            .parse()
            .ok()?;
        let revision = windows_current_version("UBR").and_then(|text| text.parse().ok());
        let release = windows_current_version("DisplayVersion");
        Some(describe_windows(build, revision, release.as_deref()))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// `Windows 11 24H2, build 26100.4351`. Windows 11 still calls itself Windows 10 in places;
/// its builds start at 22000.
#[cfg_attr(
    all(not(windows), not(test)),
    expect(dead_code, reason = "only Windows has these to describe")
)]
fn describe_windows(build: u32, revision: Option<u32>, release: Option<&str>) -> String {
    let name = if build >= 22_000 {
        "Windows 11"
    } else {
        "Windows 10"
    };
    let release = release.map_or_else(String::new, |release| format!(" {release}"));
    let revision = revision.map_or_else(String::new, |revision| format!(".{revision}"));
    format!("{name}{release}, build {build}{revision}")
}

/// A value under `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion`, as text: a string, or a
/// number in decimal.
#[cfg(windows)]
fn windows_current_version(name: &str) -> Option<String> {
    use windows::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
    };
    use windows::core::{HSTRING, w};
    let name = HSTRING::from(name);
    let key = w!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let mut text = [0_u16; 64];
    let mut size = u32::try_from(size_of_val(&text)).ok()?;
    // SAFETY: the key and value names are valid wide strings; `text` is writable for the `size`
    // bytes given, which the call updates to what it wrote, and both outlive the call.
    let read = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            &name,
            RRF_RT_REG_SZ,
            None,
            Some((&raw mut text).cast()),
            Some(&raw mut size),
        )
    };
    if read.is_ok() {
        let length = text
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(text.len());
        return Some(String::from_utf16_lossy(text.get(..length)?));
    }
    let mut number = 0_u32;
    let mut size = u32::try_from(size_of::<u32>()).ok()?;
    // SAFETY: as above, with `number` the writable space of `size` bytes.
    let read = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            &name,
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut number).cast()),
            Some(&raw mut size),
        )
    };
    read.is_ok().then(|| number.to_string())
}

#[cfg(test)]
mod tests {
    use super::{LocalTime, describe_windows, local_time_of_day, utc_from_unix, wrap_to_day};
    use std::time::Duration;

    #[test]
    fn windows_11_is_told_from_10_by_its_build() {
        assert_eq!(
            describe_windows(26_100, Some(4351), Some("24H2")),
            "Windows 11 24H2, build 26100.4351"
        );
        assert_eq!(
            describe_windows(19_045, None, None),
            "Windows 10, build 19045"
        );
    }

    #[test]
    fn unix_time_becomes_the_utc_date() {
        let at = |year, month, day, hour, minute, second| LocalTime {
            year,
            month,
            day,
            hour,
            minute,
            second,
        };
        assert_eq!(utc_from_unix(0), at(1970, 1, 1, 0, 0, 0));
        // 29 February of a leap year, and the turn of a century that is one.
        assert_eq!(utc_from_unix(951_782_400), at(2000, 2, 29, 0, 0, 0));
        assert_eq!(utc_from_unix(1_709_251_199), at(2024, 2, 29, 23, 59, 59));
        assert_eq!(utc_from_unix(1_735_689_600), at(2025, 1, 1, 0, 0, 0));
    }

    #[test]
    fn the_time_of_day_is_within_a_day() {
        assert!(local_time_of_day() < Duration::from_hours(24));
        assert_eq!(
            wrap_to_day(Duration::from_secs(25 * 3600 + 5)),
            Duration::from_secs(3600 + 5)
        );
    }
}
