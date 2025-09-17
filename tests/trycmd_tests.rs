// SPDX-FileCopyrightText: The pasejo Authors
// SPDX-License-Identifier: 0BSD

#[test]
fn cli_tests() {
    trycmd::TestCases::new()
        .case("tests/cmd/generate/*.md")
        .case("tests/cmd/help/*.md");
}
