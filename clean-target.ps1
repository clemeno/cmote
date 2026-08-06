<#
.SYNOPSIS
	Reclaims disk space from this project's cargo target directory.

.DESCRIPTION
	Cargo never garbage-collects. It writes a fresh artifact for every distinct
	build configuration it has ever seen and leaves the old one on disk forever,
	so a target directory only ever grows. Six weeks of work on cmote produced
	32 GB, of which 25 MB was source. This script reclaims that in three passes,
	ordered cheapest-first:

	  1. incremental caches   - rustc's own scratch space, pure cache, and by far
	                            the biggest single item (16 GB here). Deleting it
	                            costs one recompile of the local crate.
	  2. release artifacts    - only needed when shipping, so they are dead weight
	                            during day-to-day work (3 GB here).
	  3. duplicate deps       - the real hoard: up to 13 copies of the same crate
	                            at the same version, one per historical build
	                            (9 GB here). See Invoke-DepsPrune for how the
	                            live copy is told apart from the stale ones.

	Nothing here can lose work: every byte it deletes is a build product that
	cargo can recreate from source. The only cost of being wrong is a recompile.

	One exception to that rule lives inside the target directory and is NOT a build
	product: cmote is portable, so it writes its state in a data folder beside the
	exe, which during development means target\debug and target\release. That folder
	holds accepted host keys and the encrypted vault - things a user agreed to once
	and would have to re-approve, and a host key in particular is a security decision
	that should not silently reset. So it is preserved, and preserved by deleting
	around it rather than by copying it somewhere and putting it back: a secret that
	never moves cannot be left behind in a temporary folder if this script fails
	halfway. That is also why the release pass no longer calls `cargo clean --release`,
	which offers no way to spare a subfolder. Nothing here needs cargo on PATH.

.PARAMETER inProjectPath
	The crate root holding Cargo.toml. Defaults to the folder containing this
	script, so running it in place needs no argument.

.PARAMETER inKeepPerUnit
	How many builds of each crate-and-version to keep. The default of 2 is
	deliberate: a single crate can legitimately be built twice in one graph (a
	build-dependency copy, or two different feature sets), and the local crate
	has both a binary and a test binary under the same name. Keeping 2 covers
	those without measurably costing space. Pass 1 to be maximally aggressive at
	the price of an occasional recompile.

.PARAMETER inPreserveNames
	Folder names to spare wherever they appear in the target tree, so persisted
	state survives a cleanup. Defaults to the portable data folder cmote writes
	beside its exe. Pass an empty array to preserve nothing.

.PARAMETER inDryRun
	Report what would be deleted and stop. Always worth running once first.

.PARAMETER inSkipDeps
	Do passes 1 and 2 only. These two are risk-free: they cannot force a
	dependency to recompile.

.PARAMETER inRegistrySrc
	Also delete ~/.cargo/registry/src, the machine-wide unpacked crate sources
	(1.1 GB here). Safe: cargo re-extracts each crate on demand from the .crate
	archives it keeps beside it in registry/cache, so this needs no network.

.PARAMETER inNuke
	Skip all of the above and run plain `cargo clean`. Frees everything, and
	costs a full rebuild of every dependency. The honest option when the target
	directory is not worth reasoning about.

.EXAMPLE
	.\clean-target.ps1 -DryRun
	Show what each pass would reclaim, delete nothing.

.EXAMPLE
	.\clean-target.ps1
	The full three-pass sweep. This is the 32 GB -> 1 GB path.

.EXAMPLE
	.\clean-target.ps1 -SkipDeps
	Only the two risk-free passes, keeping the dependency cache warm so the next
	cargo check stays fast.

.NOTES
	Reads no secrets, touches no source file, and never runs a build. Safe to
	schedule.
#>

#Requires -Version 5.1

[CmdletBinding()]
param
	(
	[Alias('ProjectPath')]
	[string] $inProjectPath  = '',
	[Alias('Keep')]
	[int]    $inKeepPerUnit  = 2,
	[Alias('Preserve')]
	[string[]] $inPreserveNames = @('cmote-data'),
	[Alias('DryRun')]
	[switch] $inDryRun,
	[Alias('SkipDeps')]
	[switch] $inSkipDeps,
	[Alias('RegistrySrc')]
	[switch] $inRegistrySrc,
	[Alias('Nuke')]
	[switch] $inNuke
	)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# A cargo artifact is "<name>-<16 hex digits>.<ext>". That hash is the unit's
# identity: every file sharing it belongs to one single compilation, so it is the
# right thing to group by. Note the extension list covers MSVC's separate debug
# symbols (.pdb) and import libraries, which is where most of the weight sits.
$kArtifactPattern = '^(?<stem>.+)-(?<hash>[0-9a-f]{16})\.(?<ext>rlib|rmeta|d|dll|lib|exp|pdb|exe|so|dylib|a)$'

# Pulls "windows-sys-0.61.2" out of a path into the registry's unpacked sources.
# This is what makes the prune precise rather than a guess: it recovers the exact
# crate version a given build hash was compiled from.
$kRegistryPattern = 'registry[\\/]src[\\/][^\\/]+[\\/]([^\\/]+)[\\/]'

# Reads the crate name out of Cargo.toml, so the running-process warning below
# works for any crate this script is dropped into rather than just this one.
$kCrateNamePattern = '(?m)^\s*name\s*=\s*"([^"]+)"'

$kBytesPerGb = 1GB

<#
	Renders a byte count the way a human reads it, so the report is scannable.
#>
function Format-Size
	{
	param
		(
		[double] $inBytes
		)

	if ($inBytes -ge $kBytesPerGb) { return ('{0:N2} GB' -f ($inBytes / $kBytesPerGb)) }
	if ($inBytes -ge 1MB)          { return ('{0:N0} MB' -f ($inBytes / 1MB)) }
	return ('{0:N0} KB' -f ($inBytes / 1KB))
	}

<#
	Total size of a directory tree, or 0 when it does not exist. Errors are
	swallowed on purpose: a target directory always holds a few files locked by a
	running editor or language server, and an unreadable file must not abort a
	cleanup.
#>
function Get-TreeSize
	{
	param
		(
		[string] $inPath
		)

	if (-not (Test-Path -LiteralPath $inPath)) { return 0 }

	# Summed by hand rather than with Measure-Object, which emits nothing at all for
	# an empty directory and so has no Sum property to read - a crash under
	# Set-StrictMode, and an empty directory is exactly what a target tree is full of.
	$vTotal = [long] 0
	foreach ($vFile in (Get-ChildItem -LiteralPath $inPath -Recurse -File -Force -ErrorAction SilentlyContinue))
		{
		$vTotal += $vFile.Length
		}

	return $vTotal
	}

<#
	Deletes a directory tree and reports the bytes it held. Uses the .NET call
	rather than Remove-Item -Recurse because PowerShell's cmdlet walks the tree
	item by item, which takes minutes on an incremental cache of a hundred
	thousand files where the .NET call takes seconds.
#>
function Remove-Tree
	{
	param
		(
		[string] $inPath
		)

	if (-not (Test-Path -LiteralPath $inPath)) { return 0 }

	$vBytes = Get-TreeSize -inPath $inPath
	if ($inDryRun) { return $vBytes }

	try
		{
		[System.IO.Directory]::Delete($inPath, $true)
		}
	catch
		{
		# An incremental cache nests deep enough to pass 260 characters, which the
		# .NET call on PowerShell 5.1 refuses to touch. The shell's own rmdir has no
		# such limit, so fall back to it before giving up.
		& cmd.exe /c rmdir /s /q "$inPath" 2>&1 | Out-Null
		}

	# Report what actually went, never a number we wish were true: a file locked by
	# an editor or a running build leaves part of the tree behind.
	$vLeft = Get-TreeSize -inPath $inPath
	if ($vLeft -gt 0) { Write-Warning "$inPath : $(Format-Size $vLeft) left behind, something holds files open" }
	return ($vBytes - $vLeft)
	}

<#
	Locates the folders to spare and works out which of their parents must therefore
	survive too.

	Returns two sets: Stop holds the preserved folders themselves, never to be
	entered or deleted, and Keep holds those plus every ancestor up to the target
	root, which must survive as the path to them. Anything in neither set is a build
	product and goes.

	The search is depth-limited because a data folder sits beside an exe, one or two
	levels down, while an unbounded walk would crawl the hundred thousand directories
	of an incremental cache to learn nothing.
#>
function Get-PreservedGuard
	{
	param
		(
		[string]   $inRoot,
		[string[]] $inNames
		)

	$vGuard = [pscustomobject]@{
		Stop  = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
		Keep  = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
		Found = New-Object System.Collections.ArrayList
		}

	if ($null -eq $inNames -or $inNames.Count -eq 0) { return $vGuard }

	$vRootPath = (Resolve-Path -LiteralPath $inRoot).Path
	$vMatches  = Get-ChildItem -LiteralPath $vRootPath -Recurse -Directory -Force -Depth 3 -ErrorAction SilentlyContinue |
		Where-Object { $inNames -contains $_.Name }

	foreach ($vMatch in $vMatches)
		{
		[void] $vGuard.Stop.Add($vMatch.FullName)
		[void] $vGuard.Found.Add($vMatch.FullName)

		# Walk up to the root marking every parent, so the passes below know to step
		# through these directories instead of deleting them whole.
		$vWalk = $vMatch.FullName
		while ($vWalk -and $vWalk.Length -ge $vRootPath.Length)
			{
			[void] $vGuard.Keep.Add($vWalk)
			if ($vWalk -eq $vRootPath) { break }
			$vWalk = Split-Path -Parent $vWalk
			}
		}

	return $vGuard
	}

<#
	Empties a directory of everything except the preserved folders and the parents
	leading to them. This is the replacement for `cargo clean`, which deletes a whole
	profile directory and has no flag to spare a subfolder.

	Deleting around the data folder rather than moving it aside is the point: the
	accepted host keys and the vault never leave their directory, so no partial run
	can strand a copy of them somewhere else.
#>
function Remove-TreeExcept
	{
	param
		(
		[string] $inPath,
		[object] $inGuard
		)

	if (-not (Test-Path -LiteralPath $inPath)) { return 0 }

	$vFreed = 0
	foreach ($vChild in (Get-ChildItem -LiteralPath $inPath -Force -ErrorAction SilentlyContinue))
		{
		if ($vChild.PSIsContainer)
			{
			if ($inGuard.Stop.Contains($vChild.FullName)) { continue }
			if ($inGuard.Keep.Contains($vChild.FullName))
				{
				$vFreed += Remove-TreeExcept -inPath $vChild.FullName -inGuard $inGuard
				continue
				}

			$vFreed += Remove-Tree -inPath $vChild.FullName
			continue
			}

		$vFreed += $vChild.Length
		if (-not $inDryRun) { Remove-Item -LiteralPath $vChild.FullName -Force -ErrorAction SilentlyContinue }
		}

	return $vFreed
	}

<#
	Pass 1. Every incremental cache under the target directory, including the ones
	belonging to side target directories that editors create for their own check
	runs (rust-analyzer's flycheck, for instance). This is rustc scratch space: it
	makes a rebuild after a one-line edit fast, and nothing prunes it, so it grows
	without limit. Deleting it costs exactly one full recompile of the local crate.
#>
function Invoke-IncrementalSweep
	{
	param
		(
		[string] $inTargetPath
		)

	$vFreed = 0
	$vCaches = Get-ChildItem -LiteralPath $inTargetPath -Recurse -Directory -Force -ErrorAction SilentlyContinue |
		Where-Object { $_.Name -eq 'incremental' }

	foreach ($vCache in $vCaches)
		{
		$vFreed += Remove-Tree -inPath $vCache.FullName
		}

	return $vFreed
	}

<#
	Pass 2. Release artifacts, which matter only when shipping a build.

	Deleted by hand rather than by `cargo clean --release`, because that takes the
	whole profile directory including the data folder living beside the exe, and
	offers no way to spare it. Cargo keeps no state outside the target tree, so
	removing the files directly is equivalent.
#>
function Invoke-ReleaseClean
	{
	param
		(
		[string] $inTargetPath,
		[object] $inGuard
		)

	return (Remove-TreeExcept -inPath (Join-Path $inTargetPath 'release') -inGuard $inGuard)
	}

<#
	Reads the crate version a build hash was compiled from.

	Every compilation drops a depfile listing its sources, and for a registry crate
	those paths run through the unpacked source folder, whose name carries the exact
	version. That is the whole trick behind this pass: it lets the script tell a
	redundant rebuild of one version from two majors that genuinely coexist in the
	dependency graph, without ever asking cargo to build anything.

	Returns 'local' for the crate being developed, whose sources are not in the
	registry, and 'unknown' when there is no depfile to read.
#>
function Get-UnitVersion
	{
	param
		(
		[string] $inDepFilePath
		)

	if ([string]::IsNullOrEmpty($inDepFilePath)) { return 'unknown' }

	try
		{
		foreach ($vLine in [System.IO.File]::ReadLines($inDepFilePath))
			{
			$vMatch = [regex]::Match($vLine, $kRegistryPattern)
			if ($vMatch.Success) { return $vMatch.Groups[1].Value }
			if ($vLine.Length -gt 0) { return 'local' }
			}
		}
	catch
		{
		return 'unknown'
		}

	return 'unknown'
	}

<#
	Pass 3. The duplicate hoard.

	Cargo names an artifact after the build configuration, not the source, so a
	dependency bump or a feature change mints a new filename and abandons the old
	one. Six weeks of that leaves a dozen full copies of the same crate at the same
	version, each carrying its own debug info.

	Grouping is by crate name AND version, so majors that legitimately coexist are
	never put in competition with each other. Within a group the newest builds win
	and the rest go, along with their fingerprint directories so cargo does not keep
	bookkeeping for artifacts that are gone.

	Worst case if this deletes something still wanted: cargo recompiles that one
	crate. It cannot produce a wrong build, because cargo verifies fingerprints and
	rebuilds whatever it cannot find.
#>
function Invoke-DepsPrune
	{
	param
		(
		[string] $inTargetPath,
		[int]    $inKeep
		)

	$vDepsPath = Join-Path $inTargetPath 'debug\deps'
	if (-not (Test-Path -LiteralPath $vDepsPath)) { return 0 }

	# Collect every artifact file into one record per build hash.
	$vUnits = @{}
	foreach ($vFile in (Get-ChildItem -LiteralPath $vDepsPath -File -Force -ErrorAction SilentlyContinue))
		{
		$vMatch = [regex]::Match($vFile.Name, $kArtifactPattern)
		if (-not $vMatch.Success) { continue }

		$vHash = $vMatch.Groups['hash'].Value
		if (-not $vUnits.ContainsKey($vHash))
			{
			$vUnits[$vHash] = [pscustomobject]@{
				Hash    = $vHash
				Stem    = ''
				DepFile = ''
				Files   = New-Object System.Collections.ArrayList
				Bytes   = [long] 0
				Newest  = [datetime]::MinValue
				}
			}

		$vUnit = $vUnits[$vHash]
		[void] $vUnit.Files.Add($vFile.FullName)
		$vUnit.Bytes += $vFile.Length
		if ($vFile.LastWriteTime -gt $vUnit.Newest) { $vUnit.Newest = $vFile.LastWriteTime }

		# The depfile carries the unadorned crate name, while a library artifact wears
		# a "lib" prefix the platform linker wants. Prefer the depfile's name so both
		# spellings of one crate land in the same group.
		if ($vMatch.Groups['ext'].Value -eq 'd')
			{
			$vUnit.DepFile = $vFile.FullName
			$vUnit.Stem    = $vMatch.Groups['stem'].Value
			}
		elseif ([string]::IsNullOrEmpty($vUnit.Stem))
			{
			$vUnit.Stem = $vMatch.Groups['stem'].Value -replace '^lib', ''
			}
		}

	if ($vUnits.Count -eq 0) { return 0 }

	# Group by crate and version, then keep the newest few of each.
	$vGroups = @{}
	foreach ($vUnit in $vUnits.Values)
		{
		$vKey = '{0}|{1}' -f $vUnit.Stem, (Get-UnitVersion -inDepFilePath $vUnit.DepFile)
		if (-not $vGroups.ContainsKey($vKey)) { $vGroups[$vKey] = New-Object System.Collections.ArrayList }
		[void] $vGroups[$vKey].Add($vUnit)
		}

	$vFingerprintPath = Join-Path $inTargetPath 'debug\.fingerprint'
	$vFreed  = 0
	$vPruned = 0

	foreach ($vKey in $vGroups.Keys)
		{
		$vMembers = @($vGroups[$vKey] | Sort-Object -Property Newest -Descending)
		if ($vMembers.Count -le $inKeep) { continue }

		foreach ($vStale in $vMembers[$inKeep..($vMembers.Count - 1)])
			{
			$vFreed += $vStale.Bytes
			$vPruned++

			if (-not $inDryRun)
				{
				foreach ($vPath in $vStale.Files)
					{
					Remove-Item -LiteralPath $vPath -Force -ErrorAction SilentlyContinue
					}
				}

			# Drop cargo's bookkeeping for the unit that just went. Matched on the hash
			# alone, because the directory is named after the package while the artifact
			# is named after the crate, and the two differ whenever a package name
			# contains a dash. Outside the guard above because Remove-Tree honours the
			# dry run itself, and counting these only on the real run would make the
			# preview under-report what it is about to reclaim.
			if (Test-Path -LiteralPath $vFingerprintPath)
				{
				foreach ($vDir in (Get-ChildItem -LiteralPath $vFingerprintPath -Directory -Force -Filter "*-$($vStale.Hash)" -ErrorAction SilentlyContinue))
					{
					$vFreed += Remove-Tree -inPath $vDir.FullName
					}
				}
			}
		}

	Write-Host "  $vPruned stale builds across $($vGroups.Count) crate/version groups"

	return $vFreed
	}

<#
	The machine-wide unpacked crate sources. Shared by every Rust project on the
	box and rebuilt on demand from the .crate archives cargo keeps beside them, so
	deleting this needs no network access. Costs a re-extraction of whichever
	crates the next build touches.
#>
function Invoke-RegistrySrcClean
	{
	$vHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
	$vPath = Join-Path $vHome 'registry\src'
	return (Remove-Tree -inPath $vPath)
	}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

# Default to the folder holding the script, then to the current directory. Not
# expressed as a parameter default because PowerShell 5.1 leaves $PSScriptRoot
# empty while it is binding parameters.
$vRequested = $inProjectPath
if (-not $vRequested) { $vRequested = $PSScriptRoot }
if (-not $vRequested) { $vRequested = (Get-Location).Path }

$vProjectRoot = (Resolve-Path -LiteralPath $vRequested).Path
if (-not (Test-Path -LiteralPath (Join-Path $vProjectRoot 'Cargo.toml')))
	{
	throw "no Cargo.toml in $vProjectRoot - pass -ProjectPath pointing at the crate root"
	}

$vTargetPath = Join-Path $vProjectRoot 'target'
if (-not (Test-Path -LiteralPath $vTargetPath))
	{
	Write-Host 'nothing to do: no target directory'
	return
	}

$vBefore = Get-TreeSize -inPath $vTargetPath
Write-Host ''
Write-Host "target before: $(Format-Size $vBefore)"
if ($inDryRun) { Write-Host 'DRY RUN - nothing will be deleted' -ForegroundColor Yellow }
Write-Host ''

# A running build locks its own output. Warn rather than refuse, because deleting
# everything else is still useful and still safe.
$vCrateMatch = [regex]::Match((Get-Content -LiteralPath (Join-Path $vProjectRoot 'Cargo.toml') -Raw), $kCrateNamePattern)
$vCrateName  = if ($vCrateMatch.Success) { $vCrateMatch.Groups[1].Value } else { '' }
if ($vCrateName -and (Get-Process -Name $vCrateName -ErrorAction SilentlyContinue))
	{
	Write-Warning "$vCrateName is running: it holds its own exe and pdb open, and the next build will fail to link until you close it"
	}

# Work out what must survive before deleting anything, and say so out loud. A
# silent preservation rule is one nobody can check, and what is at stake here is
# the accepted host keys and the vault.
$vGuard = Get-PreservedGuard -inRoot $vTargetPath -inNames $inPreserveNames
if ($vGuard.Found.Count -gt 0)
	{
	Write-Host "preserving persisted state ($($inPreserveNames -join ', ')):" -ForegroundColor Cyan
	foreach ($vKept in $vGuard.Found)
		{
		Write-Host ("  {0}  {1}" -f $vKept.Substring($vProjectRoot.Length + 1), (Format-Size (Get-TreeSize -inPath $vKept))) -ForegroundColor Cyan
		}
	Write-Host ''
	}

# Each pass reports the bytes it accounted for, and they are summed rather than
# re-measured, because on a dry run nothing has moved and a second measurement of
# the tree would claim a saving of zero.
$vFreedTotal = 0

if ($inNuke)
	{
	# Everything a build produced, in both profiles, but still around the data
	# folders rather than through them.
	Write-Host 'nuke: every build product in every profile'
	$vFreedTotal = Remove-TreeExcept -inPath $vTargetPath -inGuard $vGuard
	Write-Host "  freed $(Format-Size $vFreedTotal)"
	}
else
	{
	Write-Host 'pass 1: incremental caches (rustc scratch space)'
	$vFreedIncremental = Invoke-IncrementalSweep -inTargetPath $vTargetPath
	Write-Host "  freed $(Format-Size $vFreedIncremental)"
	$vFreedTotal += $vFreedIncremental

	Write-Host 'pass 2: release artifacts (only needed when shipping)'
	$vFreedRelease = Invoke-ReleaseClean -inTargetPath $vTargetPath -inGuard $vGuard
	Write-Host "  freed $(Format-Size $vFreedRelease)"
	$vFreedTotal += $vFreedRelease

	if ($inSkipDeps)
		{
		Write-Host 'pass 3: skipped (-SkipDeps), dependency cache left warm'
		}
	else
		{
		Write-Host "pass 3: duplicate dependency builds (keeping the newest $inKeepPerUnit per crate and version)"
		$vFreedDeps = Invoke-DepsPrune -inTargetPath $vTargetPath -inKeep $inKeepPerUnit
		Write-Host "  freed $(Format-Size $vFreedDeps)"
		$vFreedTotal += $vFreedDeps
		}
	}

# Kept out of the target total on purpose: this one lives in the shared cargo home
# and belongs to every project on the machine, not to this tree.
if ($inRegistrySrc)
	{
	Write-Host 'extra: machine-wide unpacked crate sources (~/.cargo/registry/src)'
	Write-Host "  freed $(Format-Size (Invoke-RegistrySrcClean))"
	}

Write-Host ''
if ($inDryRun)
	{
	Write-Host "target would go from $(Format-Size $vBefore) to about $(Format-Size ($vBefore - $vFreedTotal))"
	Write-Host 'run again without -DryRun to apply' -ForegroundColor Yellow
	}
else
	{
	$vAfter = Get-TreeSize -inPath $vTargetPath
	Write-Host "target: $(Format-Size $vBefore) -> $(Format-Size $vAfter)  (reclaimed $(Format-Size ($vBefore - $vAfter)))"
	Write-Host 'next cargo check recompiles the local crate once; no work was lost'
	}
Write-Host ''
