const EPOCH_LENGTH: u64 = 60 * 60 * 24;

pub fn get_current_epoch(unix_time: u64) -> u64 {
    unix_time / EPOCH_LENGTH
}
