# shellcheck shell=bash
# Shared by the benchmark scripts: the terminals, their configs, and how to
# start each one with a command. Source it; don't run it.
#
# A terminal is given as `KIND[@BINARY]`, optionally labeled `LABEL=KIND[@BINARY]`:
#   jotty-cpu jotty-gpu foot kitty ghostty alacritty rio-cpu rio-gpu
# BINARY defaults to the program on PATH. For example, to compare a jotty
# build with the installed one in the same rounds:
#   new=jotty-cpu@./result/bin/jotty jotty-cpu

# Every terminal gets the same font, size, opacity and scrollback.
FONT=${FONT:-JetBrainsMono Nerd Font}
FONT_SIZE=${FONT_SIZE:-12}
# rio sizes fonts in pixels.
RIO_FONT_SIZE=${RIO_FONT_SIZE:-17}
OPACITY=${OPACITY:-0.8}
SCROLLBACK=${SCROLLBACK:-5000}
# The longest a terminal may run one measurement, in seconds.
BENCH_TIMEOUT=${BENCH_TIMEOUT:-300}

# Used by the scripts that source this file.
# shellcheck disable=SC2034
DEFAULT_TERMINALS=(jotty-cpu jotty-gpu foot kitty ghostty alacritty rio-cpu rio-gpu)

# Writes every terminal's config into $1.
write_configs() {
    local dir=$1 kind
    for kind in cpu gpu; do
        cat >"$dir/jotty-$kind.toml" <<EOF
[font]
family = "$FONT"
size = $FONT_SIZE

[terminal]
scrollback = $SCROLLBACK

[window]
opacity = $OPACITY
renderer = "$kind"
EOF
    done
    cat >"$dir/foot.ini" <<EOF
font=$FONT:size=$FONT_SIZE

[scrollback]
lines=$SCROLLBACK

[colors-dark]
alpha=$OPACITY
EOF
    cat >"$dir/kitty.conf" <<EOF
font_family $FONT
font_size $FONT_SIZE
background_opacity $OPACITY
scrollback_lines $SCROLLBACK
confirm_os_window_close 0
EOF
    cat >"$dir/ghostty.conf" <<EOF
font-family = $FONT
font-size = $FONT_SIZE
background-opacity = $OPACITY
scrollback-limit-lines = $SCROLLBACK
gtk-single-instance = false
confirm-close-surface = false
EOF
    cat >"$dir/alacritty.toml" <<EOF
[font]
normal = { family = "$FONT" }
size = $FONT_SIZE

[scrolling]
history = $SCROLLBACK

[window]
opacity = $OPACITY
EOF
    for kind in cpu gpu; do
        mkdir -p "$dir/rio-$kind"
        local cpu=false
        [ "$kind" = cpu ] && cpu=true
        cat >"$dir/rio-$kind/config.toml" <<EOF
confirm-before-quit = false
scrollback-history-limit = $SCROLLBACK

[fonts]
family = "$FONT"
size = $RIO_FONT_SIZE

[window]
opacity = $OPACITY

[navigation]
hide-if-single = true

[renderer]
use-cpu = $cpu
EOF
    done
    # jotty applies its overrides over any config; start from none.
    mkdir -p "$dir/jotty-state"
}

# The label of terminal spec $1, safe in a file name: the label given, or the
# kind with the binary's file name.
label_of() {
    local label=${1%%=*}
    if [ "$label" = "$1" ]; then
        label=${1%%@*}
        [[ $1 == *@* ]] && label+=@$(basename "${1#*@}")
    fi
    printf '%s' "${label//[^A-Za-z0-9.@_-]/_}"
}

# Fails when two of the terminal specs given get the same label.
check_labels() {
    local spec label seen=" "
    for spec in "$@"; do
        label=$(label_of "$spec")
        if [[ $seen == *" $label "* ]]; then
            echo "two terminals are labeled $label; name them with LABEL=" >&2
            return 1
        fi
        seen+="$label "
    done
}

# Runs terminal spec $1 with configs from $2, running the command that
# follows inside it, and waits for the terminal to exit.
launch() {
    local spec=${1#*=} dir=$2
    shift 2
    local kind=${spec%%@*} bin=
    [[ $spec == *@* ]] && bin=${spec#*@}
    local run=(timeout "$BENCH_TIMEOUT")
    case $kind in
        jotty-cpu | jotty-gpu)
            XDG_STATE_HOME="$dir/jotty-state" "${run[@]}" "${bin:-jotty}" \
                -c "$dir/$kind.toml" -e "$@" ;;
        foot) "${run[@]}" "${bin:-foot}" --config="$dir/foot.ini" "$@" ;;
        kitty) "${run[@]}" "${bin:-kitty}" --config "$dir/kitty.conf" "$@" ;;
        ghostty)
            "${run[@]}" "${bin:-ghostty}" --config-default-files=false \
                --config-file="$dir/ghostty.conf" -e "$@" ;;
        alacritty) "${run[@]}" "${bin:-alacritty}" --config-file "$dir/alacritty.toml" -e "$@" ;;
        rio-cpu | rio-gpu) RIO_CONFIG_HOME="$dir/$kind" "${run[@]}" "${bin:-rio}" -e "$@" ;;
        *)
            echo "unknown terminal: $kind" >&2
            return 2 ;;
    esac >/dev/null 2>&1
}

# Prints "LABEL median min max runs" for the numbers in column $3 of $2.
summarize() {
    local label=$1 file=$2 column=${3:-1}
    awk -v c="$column" '{print $c}' "$file" | sort -n | awk -v l="$label" '
        { v[NR] = $1 }
        END {
            if (NR == 0) { printf "%-24s no results\n", l; exit }
            printf "%-24s median %6d  min %6d  max %6d  runs %d\n", l, v[int((NR + 1) / 2)], v[1], v[NR], NR
        }'
}
