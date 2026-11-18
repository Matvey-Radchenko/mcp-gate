#!/bin/sh
shift 2
unit=local.mcp-gate.historical-id.service
state="$FIXTURE/manager-state"
fragment="$XDG_CONFIG_HOME/systemd/user/$unit"
case "$1" in
    list-unit-files) printf '%s\n' "$unit enabled" ;;
    list-units) printf '%s\n' "$unit loaded active running" ;;
    show)
        if [ "$2" = --property=Version ]; then printf '%s\n' '257'; exit 0; fi
        [ "$5" = "$unit" ] || exit 1
        printf '%s\n' "Id=$unit" LoadState=loaded NeedDaemonReload=no UnitFileState=enabled
        printf 'FragmentPath=%s\nDropInPaths=%s.d/override.conf\n' "$fragment" "$fragment"
        printf 'ActiveState=%s\nMainPID=0\nControlPID=0\n' "$(cat "$state")"
        ;;
    stop|start)
        [ "$2" = "$unit" ] || exit 1
        printf '%s\n' "$1" >> "$FIXTURE/manager-actions"
        if [ "$1" = start ]; then printf 'active\n' > "$state"; else printf 'inactive\n' > "$state"; fi
        ;;
    daemon-reload|reset-failed) printf '%s\n' "$1" >> "$FIXTURE/manager-actions" ;;
    *) exit 1 ;;
esac
