// Run with `node --perf-basic-prof --interpreted-frames-native-stack`: V8 then
// gives `allocation` a `JS:~allocation` entry in /tmp/perf-<pid>.map, so the
// native ArrayBuffer allocation below can be attributed to this JS function.
const ALLOCATION_SIZE = 2_000_001;

function allocation() {
  // `allocUnsafeSlow` skips the Buffer pool, so V8 asks the allocator for
  // exactly ALLOCATION_SIZE bytes.
  return Buffer.allocUnsafeSlow(ALLOCATION_SIZE);
}

allocation();
