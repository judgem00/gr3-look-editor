#!/bin/sh
# started by STARTUP.TTL through lcmd; NDMON.BIN runs on in the background
# for 3 minutes and writes its result files itself
/media/storage1/SCRIPT/RAMLOAD/NDMON.BIN > /dev/null 2>&1 &
echo NDMON_STARTED
exit 0
