// human.rs — how a byte count is spelled for a person (PLAN §17, §109).
//
// One function, in a module of its own, because it was written TWICE and the two copies did not
// agree. `ui::terminal::human_bytes` said `KiB` / `MiB` / `GiB`; `ssh::edit::human_size` did the
// same 1024-based arithmetic and labelled it `KB` / `MB` / `GB`. The same file therefore rendered
// as "1.5 KiB" in the files pane and "1.5 KB" in the message that refused to open it.
//
// The duplicate existed for a structural reason rather than carelessness: the correct one lived in
// `ui::terminal`, and `ssh::edit` will not depend on the UI layer to format a string. So the
// function moves somewhere neither layer owns, and both call it. `ui` keeps its own wrappers for
// what it adds around this (the exact byte count in parentheses, the progress readout).
//
// Binary units, one decimal above a kibibyte — enough precision for a progress readout, and no
// rounding surprises at the boundaries.

/// A byte count in the units a person reads: `0 B`, `1023 B`, `1.0 KiB`, `1.0 MiB`.
///
/// Binary throughout, and labelled binary. `KB` would be a lie about arithmetic that divides by
/// 1024, and the two spellings side by side in one program are worse than either alone.
/// The `#[expect]` covers the one conversion this module makes (§111). std has no exact
/// `u64`-to-`f64`, and above 2^53 a byte count starts rounding — which is nine petabytes, where the
/// answer is printed to one decimal place in pebibytes and the rounding is invisible by a factor of
/// billions. Below the terabyte, where every real number here lives, the conversion is exact.
#[expect(
	clippy::cast_precision_loss,
	reason = "std offers no exact u64-to-f64; exact below 2^53, and rounded to one decimal above it"
)]
pub fn bytes(count: u64) -> String {
	const KIB: f64 = 1024.0;
	let value = count as f64;
	if value < KIB {
		return format!("{count} B");
	}
	for (limit, unit) in [
		(KIB * KIB, "KiB"),
		(KIB * KIB * KIB, "MiB"),
		(KIB * KIB * KIB * KIB, "GiB"),
	] {
		if value < limit {
			return format!("{:.1} {unit}", value / (limit / KIB));
		}
	}
	format!("{:.1} TiB", value / (KIB * KIB * KIB * KIB))
}

#[cfg(test)]
mod tests {
	use super::bytes;

	#[test]
	fn a_count_below_a_kibibyte_is_exact_bytes() {
		assert_eq!(bytes(0), "0 B");
		assert_eq!(bytes(1023), "1023 B");
	}

	#[test]
	fn each_binary_boundary_steps_up_one_unit() {
		assert_eq!(bytes(1024), "1.0 KiB");
		assert_eq!(bytes(1024 * 1024), "1.0 MiB");
		assert_eq!(bytes(3 * 1024 * 1024 / 2), "1.5 MiB");
		assert_eq!(bytes(1024 * 1024 * 1024), "1.0 GiB");
		assert_eq!(bytes(5 * 1024 * 1024 * 1024), "5.0 GiB");
		assert_eq!(bytes(1024_u64.pow(4)), "1.0 TiB");
	}

	#[test]
	fn the_unit_is_always_the_binary_spelling() {
		// The whole reason this module exists: the arithmetic is binary, so the label must be too.
		// `ssh::edit::human_size` used to divide by 1024 and print `KB`.
		for count in [1024, 5 * 1024 * 1024, 3 * 1024_u64.pow(3)] {
			let spelled = bytes(count);
			assert!(
				spelled.contains("iB"),
				"{spelled} should carry a binary unit"
			);
		}
	}
}
