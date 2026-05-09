pub fn unified_diff(path: &str, actual: &str, expected: &str) -> String {
    let mut diff = String::new();
    diff.push_str("--- ");
    diff.push_str(path);
    diff.push('\n');
    diff.push_str("+++ ");
    diff.push_str(path);
    diff.push('\n');

    let actual_lines = actual.lines().collect::<Vec<_>>();
    let expected_lines = expected.lines().collect::<Vec<_>>();
    let max = actual_lines.len().max(expected_lines.len());
    for index in 0..max {
        match (actual_lines.get(index), expected_lines.get(index)) {
            (Some(left), Some(right)) if left == right => {
                diff.push(' ');
                diff.push_str(left);
                diff.push('\n');
            }
            (Some(left), Some(right)) => {
                diff.push('-');
                diff.push_str(left);
                diff.push('\n');
                diff.push('+');
                diff.push_str(right);
                diff.push('\n');
            }
            (Some(left), None) => {
                diff.push('-');
                diff.push_str(left);
                diff.push('\n');
            }
            (None, Some(right)) => {
                diff.push('+');
                diff.push_str(right);
                diff.push('\n');
            }
            (None, None) => {}
        }
    }
    diff
}
