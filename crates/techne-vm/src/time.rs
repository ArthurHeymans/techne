//! Time zones for `(techne time)`: what the time zone database knows, from
//! jiff and its bundled copy of the IANA database, so that a sandboxed
//! worker without `/usr/share/zoneinfo` and any machine get the same rules.
//! The calendar, durations and RFC 3339 are Scheme, in the library.
//!
//! An instant is an exact integer, milliseconds since the Unix epoch. A
//! zone is an IANA name, such as "Europe/Brussels" or "UTC", or a fixed
//! offset from UTC in seconds.

use std::sync::LazyLock;

use jiff::{
    Timestamp,
    civil::DateTime,
    tz::{Offset, TimeZone, TimeZoneDatabase},
};

use crate::{
    builtins::{int_arg, str_arg, type_error},
    heap::{Kind, is_kind},
    value::Value,
    vm::{Capability, Error, Vm},
};

type R = Result<Value, Error>;

fn arg(vm: &Vm, args: usize, i: usize) -> Value {
    vm.regs[args + i]
}

/// The bundled database, whatever the system has.
static DATABASE: LazyLock<TimeZoneDatabase> = LazyLock::new(TimeZoneDatabase::bundled);

fn zone_arg(vm: &Vm, v: Value, who: &str) -> Result<TimeZone, Error> {
    if is_kind(v, Kind::String) {
        let name = str_arg(vm, v, who)?;
        DATABASE.get(name).map_err(|_| Error::new(format!("{who}: no time zone named {name:?}")))
    } else if v.is_int() {
        let offset = i32::try_from(v.as_int()).ok().and_then(|s| Offset::from_seconds(s).ok());
        offset.map(TimeZone::fixed).ok_or_else(|| Error::new(format!("{who}: offset out of range: {} seconds", v.as_int())))
    } else {
        Err(type_error(who, "time zone name or offset in seconds", v))
    }
}

fn instant_arg(v: Value, who: &str) -> Result<Timestamp, Error> {
    let ms = int_arg(v, who)?;
    Timestamp::from_millisecond(ms).map_err(|_| Error::new(format!("{who}: instant out of range: {ms}")))
}

/// `(%zone-offset zone instant)`: the offset of ZONE from UTC at INSTANT,
/// in seconds east.
fn zone_offset(vm: &mut Vm, args: usize, _: usize) -> R {
    let who = "zone-offset";
    let zone = zone_arg(vm, arg(vm, args, 0), who)?;
    let at = instant_arg(arg(vm, args, 1), who)?;
    Ok(Value::int_unchecked(zone.to_offset(at).seconds() as i64))
}

/// `(%zone-instant zone year month day hour minute second millisecond)`:
/// the instant the civil time is in ZONE. A time that happens twice (the
/// clocks set back) is the earlier; one that does not happen (set
/// forward) is as long after the change as it was meant to be after the
/// time before it.
fn zone_instant(vm: &mut Vm, args: usize, _: usize) -> R {
    let who = "civil->instant";
    let zone = zone_arg(vm, arg(vm, args, 0), who)?;
    let fields = (1..8).map(|i| int_arg(arg(vm, args, i), who)).collect::<Result<Vec<i64>, Error>>()?;
    let bad = || Error::new(format!("{who}: not a date and time: {fields:?}"));
    let small = |i: usize| i8::try_from(fields[i]).map_err(|_| bad());
    let year = i16::try_from(fields[0]).map_err(|_| bad())?;
    let ns = i32::try_from(fields[6]).ok().filter(|ms| (0..1000).contains(ms)).ok_or_else(bad)? * 1_000_000;
    let dt = DateTime::new(year, small(1)?, small(2)?, small(3)?, small(4)?, small(5)?, ns).map_err(|_| bad())?;
    let at = zone.to_ambiguous_timestamp(dt).compatible().map_err(|e| Error::new(format!("{who}: {e}")))?;
    Ok(vm.make_int_raw(at.as_millisecond()))
}

/// `(%local-zone)`: the IANA name of the system's time zone.
fn local_zone(vm: &mut Vm, _: usize, _: usize) -> R {
    let who = "local-zone";
    let zone = TimeZone::try_system().map_err(|e| Error::new(format!("{who}: {e}")))?;
    let name = zone.iana_name().ok_or_else(|| Error::new(format!("{who}: the system's time zone has no IANA name")))?;
    Ok(vm.make_string_raw(name))
}

pub fn install(vm: &mut Vm) {
    crate::natives! { vm;
        "(%zone-offset zone instant)" => zone_offset;
        "(%zone-instant zone year month day hour minute second millisecond)" => zone_instant;
    }
    vm.requiring(Capability::Environment, |vm| {
        crate::natives! { vm;
            "(%local-zone)" => local_zone;
        }
    });
}
