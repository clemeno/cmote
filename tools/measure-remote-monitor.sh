#!/usr/bin/env bash
#
# measure-remote-monitor.sh — the §175 gate.
#
# PLAN.md §175 designs a Remote Monitor for cmote 5.0.0 on the premise that a
# CPU-poor, GPU-less, headless server can serve a desktop. That premise is
# unevidenced: no controlled benchmark of headless X plus VNC encoding has been
# published, and the anecdotes are bad (Xvnc over 50% CPU on a near-static
# screen under llvmpipe). This script produces the numbers that decide whether
# the feature ships at all.
#
# Run it ON THE LINUX HOST you would actually use. It needs no root, installs
# nothing, and writes only inside its own 0700 temporary directory.
#
#   ./measure-remote-monitor.sh                 # phases 0-2, no client needed
#   ./measure-remote-monitor.sh --with-client   # adds phase 3 (see warning)
#   ./measure-remote-monitor.sh --geometry 1280x720
#
# Phases 0, 1 and 2 are fully automatic. Phase 3 needs a VNC viewer driven by
# hand from your Windows machine, and is opt-in for a security reason stated
# where it starts.

set -u

# ── Constants ────────────────────────────────────────────────────────────────

kGeometry="1920x1080"    # the framebuffer to measure; §175 sizes it at half a Monitor
kDepth=24                # 24bpp is what a desktop actually runs at
kIdleSeconds=30          # long enough that a lazy timer inside Xvnc shows up
kAudioSeconds=10         # audio measured per-second, so 10s is plenty
kClientSeconds=60        # phase 3 sampling window, driven by hand
kDisplayLow=40           # probe high display numbers; :1 and :2 are commonly taken
kDisplayHigh=80
kRelayPort=5999          # phase 3 byte-counting relay
kVncPort=5901

# ── State ────────────────────────────────────────────────────────────────────

vWithClient=0
vTmpDir=""
vXvncPid=""
vDisplay=""
vRelayPid=""

# ── Plumbing ─────────────────────────────────────────────────────────────────

# Print a phase banner so the output is readable when pasted back.
say_phase() {
	printf '\n== %s ==\n\n' "$1"
}

# Print one measured fact as "label: value", aligned.
say_fact() {
	printf '  %-46s %s\n' "$1" "$2"
}

# Remove everything this script started. Registered on EXIT so a Ctrl-C does
# not leave an Xvnc behind — which is the very failure §175's three layers
# exist to prevent, and it would be poor manners to demonstrate it here.
cleanup() {
	[ -n "$vRelayPid" ] && kill "$vRelayPid" 2>/dev/null
	if [ -n "$vXvncPid" ]; then
		kill "$vXvncPid" 2>/dev/null
		# Give it a second to drop its X lock before we check for leftovers.
		sleep 1
		kill -9 "$vXvncPid" 2>/dev/null
	fi
	[ -n "$vTmpDir" ] && rm -rf "$vTmpDir"
}
trap cleanup EXIT

# Report whether a program is on PATH, and echo its path if so.
have() {
	command -v "$1" 2>/dev/null
}

# Sample one process's CPU and RSS over a window.
#   inPid      the process to watch
#   inSeconds  how long to watch it
# Prints "<percent-of-one-core> <peak-rss-kB>". Reads utime+stime from
# /proc/PID/stat (fields 14 and 15) rather than shelling out to top, so the
# number is the process's own CPU and not a sampler's guess at it.
sample_process() {
	local inPid="$1" inSeconds="$2"
	local vTicks vJiffies0 vJiffies1 vRss vRssPeak=0 vElapsed=0

	vTicks=$(getconf CLK_TCK)
	vJiffies0=$(awk '{print $14+$15}' "/proc/$inPid/stat" 2>/dev/null) || return 1

	# Poll RSS each second so a peak is caught, not just the closing value.
	while [ "$vElapsed" -lt "$inSeconds" ]; do
		sleep 1
		vElapsed=$((vElapsed + 1))
		vRss=$(awk '/^VmRSS:/{print $2}' "/proc/$inPid/status" 2>/dev/null)
		[ -n "${vRss:-}" ] && [ "$vRss" -gt "$vRssPeak" ] && vRssPeak="$vRss"
	done

	vJiffies1=$(awk '{print $14+$15}' "/proc/$inPid/stat" 2>/dev/null) || return 1
	awk -v a="$vJiffies0" -v b="$vJiffies1" -v t="$vTicks" \
	    -v s="$inSeconds" -v r="$vRssPeak" \
	    'BEGIN { printf "%.1f %d", (b - a) / t / s * 100, r }'
}

# Time a whole pipeline and report its CPU cost as a multiple of realtime.
#   inSeconds  how many seconds of audio the pipeline processed
#   inLabel    what to call it in the output
#   remaining arguments: the command, run through sh -c
# A ×realtime figure is the only portable way to compare this across machines,
# and it is what the published FLAC comparisons use.
time_pipeline() {
	local inSeconds="$1" inLabel="$2"
	shift 2
	local vOut vUser vSys vReal vCpu vRatio

	TIMEFORMAT='%3U %3S %3R'
	vOut=$( { time sh -c "$*" >/dev/null 2>/dev/null; } 2>&1 | tail -1 )
	vUser=$(echo "$vOut" | awk '{print $1}')
	vSys=$(echo "$vOut" | awk '{print $2}')
	vReal=$(echo "$vOut" | awk '{print $3}')

	vCpu=$(awk -v u="$vUser" -v s="$vSys" -v r="$vReal" \
		'BEGIN { if (r > 0) printf "%.1f", (u + s) / r * 100; else print "n/a" }')
	vRatio=$(awk -v a="$inSeconds" -v r="$vReal" \
		'BEGIN { if (r > 0) printf "%.0f", a / r; else print 0 }')

	say_fact "$inLabel CPU (% of one core)" "$vCpu"
	say_fact "$inLabel speed (x realtime)" "${vRatio}x"
}

# Find a display number nothing is using, by the same two tests the vncserver
# script uses: the X lock file and the X socket.
find_free_display() {
	local vN="$kDisplayLow"
	while [ "$vN" -le "$kDisplayHigh" ]; do
		if [ ! -e "/tmp/.X${vN}-lock" ] && [ ! -e "/tmp/.X11-unix/X${vN}" ]; then
			echo "$vN"
			return 0
		fi
		vN=$((vN + 1))
	done
	return 1
}

# ── Arguments ────────────────────────────────────────────────────────────────

while [ $# -gt 0 ]; do
	case "$1" in
		--with-client) vWithClient=1 ;;
		--geometry)    shift; kGeometry="$1" ;;
		-h|--help)     sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
		*)             echo "unknown argument: $1" >&2; exit 2 ;;
	esac
	shift
done

printf 'measure-remote-monitor.sh — the PLAN.md §175 gate\n'
printf 'host: %s   kernel: %s   %s\n' "$(uname -n)" "$(uname -r)" "$(date -Is)"

# ── Phase 0: which of §175's three worlds is this host in? ───────────────────

say_phase "Phase 0 — what is actually installed"

vXvnc=$(have Xvnc)
vXvfb=$(have Xvfb)
vXorg=$(have Xorg)
vFlac=$(have flac)
vParec=$(have parec)
vPwRecord=$(have pw-record)
vPactl=$(have pactl)
vPython=$(have python3)

say_fact "Xvnc"        "${vXvnc:-ABSENT}"
say_fact "Xvfb"        "${vXvfb:-absent}"
say_fact "Xorg"        "${vXorg:-absent}"
say_fact "flac"        "${vFlac:-absent (raw PCM fallback would be used)}"
say_fact "parec"       "${vParec:-absent}"
say_fact "pw-record"   "${vPwRecord:-absent}"
say_fact "pactl"       "${vPactl:-absent}"
say_fact "python3"     "${vPython:-absent (phase 3 unavailable)}"

if [ -n "$vXvnc" ]; then
	say_fact "Xvnc version" "$("$vXvnc" -version 2>&1 | head -1)"
fi

# libX11 decides whether there can be any graphical application at all — which
# is §175's distinction between a host that merely lacks a display server and
# one where the feature is impossible.
vLibX11=$(ldconfig -p 2>/dev/null | awk '/libX11\.so\.6/ { print $NF; exit }')
say_fact "libX11.so.6" "${vLibX11:-ABSENT}"

# Something has to be run inside the framebuffer for the load phases to mean
# anything. Report what is available rather than assuming a desktop exists.
vClients=""
for vProg in xterm xeyes xclock xlogo x11perf glxgears firefox; do
	vFound=$(have "$vProg")
	[ -n "$vFound" ] && vClients="$vClients $vProg"
done
say_fact "X clients found" "${vClients:- NONE}"

# Every finding prints before the one verdict that can stop the run, so a host
# missing two things learns both in one pass instead of one per install.
printf '\n'
if [ -z "$vLibX11" ]; then
	echo "  VERDICT: no libX11. This is §175's third world — there is nothing"
	echo "           graphical to display, so the feature is impossible here by"
	echo "           design, not by omission."
fi
if [ -z "$vClients" ]; then
	echo "  NOTE: no X clients found, so the 'under load' numbers cannot be"
	echo "        produced on this host. Phase 1's idle figures still stand,"
	echo "        and they are the ones that gate the feature. 'xterm' is the"
	echo "        package name on every family below and is the smallest thing"
	echo "        that draws."
fi
if [ -z "$vXvnc" ]; then
	echo "  VERDICT: no Xvnc. §175 refuses to install a display server, so that"
	echo "           is the admin's decision and not this script's:"
	echo "             RHEL / CentOS / Fedora   tigervnc-server"
	echo "                                      (tigervnc-server-minimal is the"
	echo "                                       Xvnc-only subpackage)"
	echo "             Debian / Ubuntu          tigervnc-standalone-server"
	echo "             Arch                     tigervnc"
	echo "           No phase after this one ran."
	exit 1
fi

# ── Phase 1: Xvnc alone, nobody attached ─────────────────────────────────────
#
# This is the number that decides everything. If a framebuffer nobody is
# looking at already costs half a core, the feature is answered.

say_phase "Phase 1 — Xvnc idle, no client attached"

vTmpDir=$(mktemp -d) || { echo "cannot create temp dir" >&2; exit 1; }
chmod 700 "$vTmpDir"
vDisplay=$(find_free_display) || { echo "no free display number" >&2; exit 1; }
vSocket="$vTmpDir/vnc-$$-$RANDOM.sock"

say_fact "display" ":$vDisplay"
say_fact "geometry" "$kGeometry x ${kDepth}bpp"
say_fact "socket" "$vSocket"

# §175's exact posture: a 0600 socket inside a 0700 directory, no TCP at all.
# -rfbport -1 is the load-bearing flag — -rfbunixpath alone does NOT disable
# TCP, because TigerVNC 1.12.0 deliberately made the Unix servers listen on
# both. -nolisten tcp closes the X server's own port for the same reason.
"$vXvnc" ":$vDisplay" \
	-rfbunixpath "$vSocket" \
	-rfbunixmode 0600 \
	-rfbport -1 \
	-SecurityTypes None \
	-geometry "$kGeometry" \
	-depth "$kDepth" \
	-MaxDisconnectionTime 0 \
	-nolisten tcp \
	-Log '*:stderr:30' \
	>"$vTmpDir/xvnc.log" 2>&1 &
vXvncPid=$!

sleep 3
if ! kill -0 "$vXvncPid" 2>/dev/null; then
	echo "  Xvnc exited immediately. Its log:"
	sed 's/^/    /' "$vTmpDir/xvnc.log"
	exit 1
fi

# The §175 runnable check, run here for real: a flag that is silently ignored is
# the failure mode, so verify the ABSENCE of a listening port rather than the
# success of the command. TigerVNC issue #1374 reports 1.12.x regressing
# Unix-only listening, and whether 1.16.x still does is unconfirmed.
vTcpFound="none"
if have ss >/dev/null; then
	vTcpFound=$(ss -ltnp 2>/dev/null | grep -c "pid=$vXvncPid," )
elif have netstat >/dev/null; then
	vTcpFound=$(netstat -ltnp 2>/dev/null | grep -c "$vXvncPid/" )
else
	vTcpFound="unknown (no ss or netstat)"
fi
say_fact "TCP ports held by Xvnc (must be 0)" "$vTcpFound"
if [ "$vTcpFound" != "0" ] && [ "$vTcpFound" != "none" ] \
   && [ "$vTcpFound" != "unknown (no ss or netstat)" ]; then
	echo "  *** SECURITY FINDING: -rfbport -1 did not take effect on this build."
	echo "  *** §175's posture depends on it. Do not ship against this version"
	echo "  *** without re-reading TigerVNC issue #1374."
fi

say_fact "socket mode (must be 600)" "$(stat -c '%a' "$vSocket" 2>/dev/null || echo '?')"
say_fact "dir mode (must be 700)" "$(stat -c '%a' "$vTmpDir" 2>/dev/null || echo '?')"

printf '\n  sampling %ss...\n' "$kIdleSeconds"
vResult=$(sample_process "$vXvncPid" "$kIdleSeconds")
say_fact "idle CPU (% of one core)" "$(echo "$vResult" | awk '{print $1}')"
say_fact "idle RSS (kB)" "$(echo "$vResult" | awk '{print $2}')"
say_fact "idle RSS (MiB)" "$(echo "$vResult" | awk '{printf "%.1f", $2/1024}')"

# ── Phase 2: the sound path ──────────────────────────────────────────────────
#
# The two bounds need no audio hardware and no audio daemon at all: digital
# silence is FLAC's best case and white noise is its worst. Between them they
# bracket every real desktop sound, which is why they are measured first.

say_phase "Phase 2 — the sound path"

if [ -z "$vFlac" ]; then
	echo "  flac absent. §175 falls back to raw PCM, which costs"
	echo "  192 kB/s at 48 kHz stereo before SSH's deflate (§167)."
else
	vRate=48000
	vChan=2
	vBytes=$((vRate * vChan * 2 * kAudioSeconds))
	vFlacArgs="--force-raw-format --endian=little --sign=signed --channels=$vChan --bps=16 --sample-rate=$vRate -0 -c -"

	say_fact "flac version" "$("$vFlac" --version 2>&1 | head -1)"
	say_fact "test material" "${kAudioSeconds}s @ ${vRate}Hz x${vChan} s16 = $vBytes bytes"

	printf '\n  silence (FLAC best case — an idle desktop):\n'
	time_pipeline "$kAudioSeconds" "    silence" \
		"head -c $vBytes /dev/zero | '$vFlac' $vFlacArgs"
	vSize=$(head -c "$vBytes" /dev/zero | "$vFlac" $vFlacArgs 2>/dev/null | wc -c)
	say_fact "    silence compressed size (bytes)" "$vSize"
	say_fact "    silence ratio" \
		"$(awk -v a="$vSize" -v b="$vBytes" 'BEGIN{printf "%.2f%%", a/b*100}')"

	printf '\n  white noise (FLAC worst case — incompressible):\n'
	time_pipeline "$kAudioSeconds" "    noise" \
		"head -c $vBytes /dev/urandom | '$vFlac' $vFlacArgs"
	vSize=$(head -c "$vBytes" /dev/urandom | "$vFlac" $vFlacArgs 2>/dev/null | wc -c)
	say_fact "    noise compressed size (bytes)" "$vSize"
	say_fact "    noise ratio" \
		"$(awk -v a="$vSize" -v b="$vBytes" 'BEGIN{printf "%.2f%%", a/b*100}')"
fi

# The capture half needs a real sink. §175 offers to create a null sink and
# never does so silently, so this script only reports the command.
printf '\n'
if [ -n "$vPactl" ]; then
	vSink=$("$vPactl" get-default-sink 2>/dev/null)
	if [ -n "${vSink:-}" ] && [ "$vSink" != "@DEFAULT_SINK@" ]; then
		say_fact "default sink" "$vSink"
		vSinkRate=$("$vPactl" list sinks 2>/dev/null \
			| awk '/Sample Specification/ { print $0; exit }')
		say_fact "sink format (capture at THIS, never resample)" "${vSinkRate:-unknown}"

		if [ -n "$vParec" ]; then
			printf '\n  parec from the sink monitor, %ss:\n' "$kAudioSeconds"
			"$vParec" -d "${vSink}.monitor" --format=s16le \
				--rate=48000 --channels=2 >/dev/null 2>&1 &
			vParecPid=$!
			sleep 1
			if kill -0 "$vParecPid" 2>/dev/null; then
				vResult=$(sample_process "$vParecPid" "$kAudioSeconds")
				say_fact "    parec CPU (% of one core)" \
					"$(echo "$vResult" | awk '{print $1}')"
				say_fact "    parec RSS (kB)" \
					"$(echo "$vResult" | awk '{print $2}')"
			else
				echo "    parec exited immediately — no monitor source?"
			fi
			kill "$vParecPid" 2>/dev/null
		fi
	else
		echo "  No sink. §175 would OFFER this command and never run it silently:"
		echo "      pactl load-module module-null-sink sink_name=cmote_stream"
		echo "  A null sink is clocked by system time, so it needs no hardware."
	fi
else
	echo "  No pactl. No PulseAudio or pipewire-pulse on this host, so the"
	echo "  capture half cannot be measured. The FLAC bounds above still hold."
fi

# ── Phase 3: with a viewer attached ──────────────────────────────────────────

if [ "$vWithClient" -eq 0 ]; then
	say_phase "Phase 3 — skipped"
	echo "  Pass --with-client to measure CPU and bytes-per-encoding with a"
	echo "  viewer attached. It is opt-in because it must expose a loopback TCP"
	echo "  port, and a loopback port is reachable by EVERY local account on"
	echo "  this machine — the exposure §175 exists to avoid. Only do it on a"
	echo "  box where you are the only user."
else
	say_phase "Phase 3 — with a viewer attached (loopback TCP, see warning)"

	if [ -z "$vPython" ]; then
		echo "  python3 absent; the byte-counting relay cannot run. Skipping."
	else
		echo "  *** This phase puts Xvnc on 127.0.0.1:$kVncPort with no"
		echo "  *** authentication. Any local account can connect for as long"
		echo "  *** as it runs. This is a MEASUREMENT posture and is explicitly"
		echo "  *** not what §175 ships."
		printf '\n'

		kill "$vXvncPid" 2>/dev/null
		sleep 1
		vXvncPid=""

		"$vXvnc" ":$vDisplay" \
			-rfbport "$kVncPort" \
			-localhost \
			-SecurityTypes None \
			-geometry "$kGeometry" \
			-depth "$kDepth" \
			-nolisten tcp \
			-Log '*:stderr:100' \
			>"$vTmpDir/xvnc-client.log" 2>&1 &
		vXvncPid=$!
		sleep 3

		# A relay rather than a packet counter: it gives an unambiguous
		# server-to-client byte total, and needs nothing but python3.
		cat >"$vTmpDir/relay.py" <<'PYEOF'
import socket, sys, threading
listen_port, target_port = int(sys.argv[1]), int(sys.argv[2])
down = [0]
def pump(src, dst, count):
	try:
		while True:
			b = src.recv(65536)
			if not b:
				break
			if count:
				down[0] += len(b)
			dst.sendall(b)
	except Exception:
		pass
	finally:
		try: dst.shutdown(socket.SHUT_WR)
		except Exception: pass
srv = socket.socket()
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(("127.0.0.1", listen_port))
srv.listen(1)
while True:
	c, _ = srv.accept()
	s = socket.create_connection(("127.0.0.1", target_port))
	threading.Thread(target=pump, args=(c, s, False), daemon=True).start()
	t = threading.Thread(target=pump, args=(s, c, True), daemon=True)
	t.start()
	t.join()
	print(down[0], flush=True)
	down[0] = 0
PYEOF
		"$vPython" "$vTmpDir/relay.py" "$kRelayPort" "$kVncPort" \
			>"$vTmpDir/bytes.txt" 2>/dev/null &
		vRelayPid=$!
		sleep 1

		echo "  From your Windows machine, in a second terminal:"
		echo ""
		echo "      ssh -L $kRelayPort:127.0.0.1:$kRelayPort $(whoami)@$(uname -n)"
		echo ""
		echo "  then point a viewer at 127.0.0.1:$kRelayPort, once per encoding:"
		echo ""
		echo "      vncviewer -PreferredEncoding=ZRLE  127.0.0.1:$kRelayPort"
		echo "      vncviewer -PreferredEncoding=Tight 127.0.0.1:$kRelayPort"
		echo ""
		echo "  While each is connected, drive the workload: drag a window"
		echo "  around for 20s, scroll a wall of text for 20s, play a video for"
		echo "  20s. Then DISCONNECT — the byte total prints on disconnect."
		echo ""
		printf '  sampling Xvnc for %ss. Connect now.\n\n' "$kClientSeconds"

		vResult=$(sample_process "$vXvncPid" "$kClientSeconds")
		say_fact "Xvnc CPU under load (% of one core)" \
			"$(echo "$vResult" | awk '{print $1}')"
		say_fact "Xvnc RSS under load (kB)" \
			"$(echo "$vResult" | awk '{print $2}')"

		printf '\n  server-to-client bytes, one line per completed session:\n'
		if [ -s "$vTmpDir/bytes.txt" ]; then
			sed 's/^/    /' "$vTmpDir/bytes.txt"
		else
			echo "    (none — no viewer disconnected during the window)"
		fi

		printf '\n  Xvnc own statistics, from its log:\n'
		grep -iE 'stat|rect|bytes|encod|throughput' \
			"$vTmpDir/xvnc-client.log" 2>/dev/null \
			| sed 's/^/    /' | head -40
	fi
fi

# ── Verdict ──────────────────────────────────────────────────────────────────

say_phase "What to read out of this"

cat <<'EOF'
  Phase 1's idle CPU is the gate. §175 is designed for a host where a
  framebuffer nobody is looking at is nearly free. If idle CPU is a few
  percent, the design holds. If it is tens of percent, that is the answer,
  and it is much cheaper to have learned it now.

  Phase 1's idle RSS sets the cost of the "keep it running" option, which is
  what the per-target checkbox and -MaxDisconnectionTime trade against.

  Phase 2's two FLAC ratios bracket every real sound. If silence encodes to
  almost nothing and the CPU is low, FLAC earns its place; if the noise case
  is expensive, raw PCM plus SSH's existing deflate (§167) is enough.

  Phase 3's bytes-per-encoding decides whether Tight's JPEG path — and the
  second narrowing of §41 that it costs — is worth having at all.

  Paste this whole output back.
EOF

printf '\n'
