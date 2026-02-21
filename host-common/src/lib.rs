#![cfg_attr(not(test), no_std)]

#[inline(always)]
pub fn serpentine_index(x: usize, y: usize, width: usize, height: usize) -> usize {
    let py = height - 1 - y; // flip: framebuffer top-left → physical bottom-left
    if py.is_multiple_of(2) {
        // Even physical rows (0, 2, ...): left to right
        py * width + x
    } else {
        // Odd physical rows (1, 3, ...): right to left
        py * width + (width - 1 - x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serpentine_index() {
        // Starts at the bottom-left, goes right on the bottom row, then left on the second row up, and so on...
        assert_eq!(serpentine_index(0, 0, 16, 16), 255);
        assert_eq!(serpentine_index(15, 0, 16, 16), 240);
        assert_eq!(serpentine_index(15, 1, 16, 16), 239);
        assert_eq!(serpentine_index(15, 14, 16, 16), 16);
        assert_eq!(serpentine_index(0, 15, 16, 16), 0);
        assert_eq!(serpentine_index(15, 15, 16, 16), 15);
    }
}
