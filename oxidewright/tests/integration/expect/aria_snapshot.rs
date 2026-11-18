//! Port of `to-match-aria-snapshot.spec.ts` (a selection).
//!
//! Skipped:
//! * `AR-119`..`AR-124` attribute tables beyond the `checked` cases ported
//!   below, and `AR-128 should unpack escaped names`: they exercise the
//!   injected template matcher, shared with `locator.aria_snapshot()`.
//! * The second case of `AR-129` and the later cases of `AR-130`: the YAML
//!   is parsed by `yaml-rust2`, whose syntax error messages differ from the
//!   `yaml` package's.
//! * `AR-140`, `AR-142`: rely on ARIA snapshot changes newer than the
//!   bundled Patchright 1.64.3 injected script.
//! * Snapshot files (`toMatchAriaSnapshot({ name })`) and baseline
//!   generation: they belong to the test runner.

use oxidewright::expect::expect;

use super::*;
use crate::common::*;

#[test]
fn should_match_page() {
    run(|t| async move {
        t.page.set_content("<h1>title</h1>").await.unwrap();
        expect(&t.page)
            .to_match_aria_snapshot("\n    - heading \"title\"\n  ")
            .await
            .unwrap();
    });
}

#[test]
fn should_match_page_complex() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<h1>Microsoft</h1>
    <div>Open source projects and samples from Microsoft</div>
    <ul>
      <li>
        <a href="about:blank">Playwright</a>
      </li>
    </ul>"#,
            )
            .await
            .unwrap();
        expect(&t.page)
            .to_match_aria_snapshot(
                r#"
    - heading "Microsoft"
    - text: Open source projects and samples from Microsoft
    - list:
      - listitem:
        - link "Playwright"
  "#,
            )
            .await
            .unwrap();
    });
}

#[test]
fn should_match_page_with_not() {
    run(|t| async move {
        t.page.set_content("<h1>title</h1>").await.unwrap();
        expect(&t.page)
            .not()
            .to_match_aria_snapshot("- heading \"wrong\"")
            .await
            .unwrap();
    });
}

#[test]
fn should_fail_page_with_timeout() {
    run(|t| async move {
        t.page.set_content("<h1>title</h1>").await.unwrap();
        let m = message(
            expect(&t.page)
                .to_match_aria_snapshot("- heading \"wrong\"")
                .timeout(2000)
                .await,
        );
        assert_contains(&m, "expect(page).toMatchAriaSnapshot(expected) failed");
        assert_contains(&m, "Timeout:  2000ms");
        assert_not_contains(&m, "Locator");
    });
}

#[test]
fn should_match_in_list() {
    run(|t| async move {
        t.page
            .set_content("<h1>title</h1><h1>title 2</h1>")
            .await
            .unwrap();
        expect(&t.page)
            .to_match_aria_snapshot("- heading \"title\"")
            .await
            .unwrap();
    });
}

#[test]
fn should_match_list_with_accessible_name() {
    run(|t| async move {
        t.page
            .set_content("<ul aria-label=\"my list\"><li>one</li><li>two</li></ul>")
            .await
            .unwrap();
        expect(&t.page)
            .to_match_aria_snapshot(
                r#"
    - list "my list":
      - listitem: "one"
      - listitem: "two"
  "#,
            )
            .await
            .unwrap();
    });
}

#[test]
fn should_match_deep_item() {
    run(|t| async move {
        t.page
            .set_content("<div><h1>title</h1><h1>title 2</h1></div>")
            .await
            .unwrap();
        expect(&t.page)
            .to_match_aria_snapshot("- heading \"title\"")
            .await
            .unwrap();
    });
}

#[test]
fn should_match_complex() {
    run(|t| async move {
        t.page
            .set_content("<ul><li><a href='about:blank'>link</a></li></ul>")
            .await
            .unwrap();
        expect(&t.page)
            .to_match_aria_snapshot("- list:\n  - listitem:\n    - link \"link\"")
            .await
            .unwrap();
    });
}

#[test]
fn should_match_regex() {
    run(|t| async move {
        t.page.set_content("<h1>Issues 12</h1>").await.unwrap();
        expect(&t.page)
            .to_match_aria_snapshot(r"- heading /Issues \d+/")
            .await
            .unwrap();
        t.page.set_content("<h1>Issues 1/2</h1>").await.unwrap();
        expect(&t.page)
            .to_match_aria_snapshot(r"- heading /Issues 1[/]2/")
            .await
            .unwrap();
        t.page.set_content("<h1>Issues 1[</h1>").await.unwrap();
        expect(&t.page)
            .to_match_aria_snapshot(r"- heading /Issues 1\[/")
            .await
            .unwrap();
        t.page.set_content("<h1>Issues 1]]2</h1>").await.unwrap();
        expect(&t.page)
            .to_match_aria_snapshot(r"- heading /Issues 1[\]]]2/")
            .await
            .unwrap();
    });
}

#[test]
fn should_allow_text_nodes() {
    run(|t| async move {
        t.page
            .set_content(
                "<h1>Microsoft</h1><div>Open source projects and samples from Microsoft</div>",
            )
            .await
            .unwrap();
        expect(&t.page)
            .to_match_aria_snapshot(
                "- heading \"Microsoft\"\n- text: \"Open source projects and samples from Microsoft\"",
            )
            .await
            .unwrap();
    });
}

#[test]
fn details_visibility() {
    run(|t| async move {
        t.page
            .set_content("<details><summary>Summary</summary><div>Details</div></details>")
            .await
            .unwrap();
        expect(&t.page)
            .to_match_aria_snapshot("- group: \"Summary\"")
            .await
            .unwrap();
    });
}

#[test]
fn checked_attribute() {
    run(|t| async move {
        t.page
            .set_content("<input type='checkbox' checked />")
            .await
            .unwrap();
        expect(&t.page)
            .to_match_aria_snapshot("- checkbox")
            .await
            .unwrap();
        expect(&t.page)
            .to_match_aria_snapshot("- checkbox [checked]")
            .await
            .unwrap();
        expect(&t.page)
            .to_match_aria_snapshot("- checkbox [checked=true]")
            .await
            .unwrap();
        for template in ["- checkbox [checked=false]", "- checkbox [checked=mixed]"] {
            let m = message(
                expect(&t.page)
                    .to_match_aria_snapshot(template)
                    .timeout(1000)
                    .await,
            );
            assert_contains(&m, "expect(page).toMatchAriaSnapshot(expected) failed");
            assert_contains(&m, "Timeout:  1000ms");
        }
        let m = message(
            expect(&t.page)
                .to_match_aria_snapshot("- checkbox [checked=5]")
                .timeout(1000)
                .await,
        );
        assert_contains(&m, " attribute must be a boolean or \"mixed\"");
    });
}

#[test]
fn integration_test_2() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<div>
      <header>
        <h1>todos</h1>
        <input placeholder="What needs to be done?">
      </header>
    </div>"#,
            )
            .await
            .unwrap();
        expect(&t.page)
            .to_match_aria_snapshot("- heading \"todos\"\n- textbox \"What needs to be done?\"")
            .await
            .unwrap();
    });
}

#[test]
fn expected_formatter() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<div>
      <header>
        <h1>todos</h1>
        <input placeholder="What needs to be done?">
        <button>Time 15:30</button>
      </header>
    </div>"#,
            )
            .await
            .unwrap();
        let m = message(
            expect(&t.page)
                .to_match_aria_snapshot(
                    "\n    - heading \"todos\"\n    - textbox \"Wrong text\"\n  ",
                )
                .timeout(1)
                .await,
        );
        assert_contains(
            &m,
            "expect(page).toMatchAriaSnapshot(expected) failed\n\nTimeout: 1ms\n- Expected  - 2\n+ Received  + 4\n\n- - heading \"todos\"\n- - textbox \"Wrong text\"\n+ - banner:\n+   - heading \"todos\" [level=1]\n+   - textbox \"What needs to be done?\"\n+   - button \"Time 15:30\"\n\nCall log:\n",
        );
    });
}

#[test]
fn should_report_error_in_yaml() {
    run(|t| async move {
        t.page.set_content("<h1>title</h1>").await.unwrap();
        let m = message(
            expect(&t.page)
                .to_match_aria_snapshot("\n      heading \"title\"\n    ")
                .await,
        );
        assert_eq!(
            m,
            "expect(page).toMatchAriaSnapshot(expected) failed\n\nExpected: \"heading \\\"title\\\"\"\nError: Aria snapshot must be a YAML sequence, elements starting with \" -\"\n\nCall log:\n  - Expect \"toMatchAriaSnapshot\" with timeout 5000ms\n"
        );
    });
}

#[test]
fn should_report_error_in_yaml_keys() {
    run(|t| async move {
        t.page.set_content("<h1>title</h1>").await.unwrap();
        let m = message(
            expect(&t.page)
                .to_match_aria_snapshot("\n      - heading \"title\n    ")
                .await,
        );
        assert_eq!(
            m,
            "expect(page).toMatchAriaSnapshot(expected) failed\n\nExpected: \"- heading \\\"title\"\nError: Unterminated string:\n\nheading \"title\n              ^\n\nCall log:\n  - Expect \"toMatchAriaSnapshot\" with timeout 5000ms\n"
        );
    });
}

#[test]
fn call_log_should_contain_actual_snapshot() {
    run(|t| async move {
        t.page.set_content("<h1>todos</h1>").await.unwrap();
        let m = message(
            expect(&t.page)
                .to_match_aria_snapshot("- heading \"wrong\"")
                .timeout(3000)
                .await,
        );
        assert_contains(&m, "unexpected value \"- heading \"todos\" [level=1]\"");
    });
}

#[test]
fn should_parse_attributes() {
    run(|t| async move {
        t.page
            .set_content("<button aria-pressed=\"mixed\">hello world</button>")
            .await
            .unwrap();
        expect(&t.page)
            .to_match_aria_snapshot("- button [pressed=mixed ]")
            .await
            .unwrap();
        t.page.set_content("<h2>hello world</h2>").await.unwrap();
        expect(&t.page.locator("body"))
            .not()
            .to_match_aria_snapshot("- heading [level =  -3 ]")
            .await
            .unwrap();
    });
}

#[test]
fn should_not_unshift_actual_template_text() {
    run(|t| async move {
        t.page
            .set_content("<h1>title</h1><h1>title 2</h1>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page)
                .to_match_aria_snapshot(
                    "\n        - heading \"title\" [level=1]\n    - heading \"title 2\" [level=1]\n  ",
                )
                .timeout(1000)
                .await,
        );
        // Playwright finds the text in the `yaml` package's error excerpt;
        // `yaml-rust2` does not quote the source, so check the expected value.
        assert_contains(
            &m,
            r#"Expected: "    - heading \"title\" [level=1]
- heading \"title 2\" [level=1]""#,
        );
    });
}

#[test]
fn should_not_match_what_is_not_matched() {
    run(|t| async move {
        t.page.set_content("<p>Text</p>").await.unwrap();
        let m = message(
            expect(&t.page)
                .to_match_aria_snapshot("- paragraph:\n  - button \"bogus\"")
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "\n- - paragraph:\n-   - button \"bogus\"\n+ - paragraph: Text",
        );
    });
}

#[test]
fn should_match_url() {
    run(|t| async move {
        t.page
            .set_content("<a href='https://example.com'>Link</a>")
            .await
            .unwrap();
        expect(&t.page)
            .to_match_aria_snapshot("- link:\n  - /url: /.*example.com/")
            .await
            .unwrap();
    });
}

#[test]
fn should_detect_unexpected_children_equal() {
    run(|t| async move {
        t.page
            .set_content("<ul><li>One</li><li>Two</li><li>Three</li></ul>")
            .await
            .unwrap();
        expect(&t.page)
            .to_match_aria_snapshot("- list:\n  - listitem: \"One\"\n  - listitem: \"Three\"")
            .await
            .unwrap();
        let m = message(
            expect(&t.page)
                .to_match_aria_snapshot(
                    "- list:\n  - /children: equal\n  - listitem: \"One\"\n  - listitem: \"Three\"",
                )
                .timeout(1000)
                .await,
        );
        assert_contains(&m, "expect(page).toMatchAriaSnapshot(expected) failed");
        assert_contains(&m, "+   - listitem: Two");
    });
}

const NESTED_LIST: &str = "<ul><li><ul><li>1.1</li><li>1.2</li></ul></li></ul>";

#[test]
fn should_detect_unexpected_children_deep_equal() {
    run(|t| async move {
        t.page.set_content(NESTED_LIST).await.unwrap();
        expect(&t.page)
            .to_match_aria_snapshot("- list:\n  - listitem:\n    - list:\n      - listitem: 1.1")
            .await
            .unwrap();
        expect(&t.page)
            .to_match_aria_snapshot(
                "- list:\n  - /children: equal\n  - listitem:\n    - list:\n      - listitem: 1.1",
            )
            .await
            .unwrap();
        let m = message(
            expect(&t.page)
                .to_match_aria_snapshot(
                    "- list:\n  - /children: deep-equal\n  - listitem:\n    - list:\n      - listitem: 1.1",
                )
                .timeout(1000)
                .await,
        );
        assert_contains(&m, "expect(page).toMatchAriaSnapshot(expected) failed");
        assert_contains(&m, "+       - listitem: \"1.2\"");
    });
}

#[test]
fn should_allow_restoring_contain_mode_inside_deep_equal() {
    run(|t| async move {
        t.page.set_content(NESTED_LIST).await.unwrap();
        expect(&t.page)
            .to_match_aria_snapshot(
                "- list:\n  - /children: deep-equal\n  - listitem:\n    - list:\n      - /children: contain\n      - listitem: 1.1",
            )
            .await
            .unwrap();
    });
}

#[test]
fn top_level_deep_equal() {
    run(|t| async move {
        t.page.set_content(NESTED_LIST).await.unwrap();
        let m = message(
            expect(&t.page)
                .to_match_aria_snapshot("- /children: deep-equal\n- list")
                .timeout(1000)
                .await,
        );
        assert_contains(
            &m,
            "- - /children: deep-equal\n- - list\n+ - list:\n+   - listitem:\n+     - list:\n+       - listitem: \"1.1\"\n+       - listitem: \"1.2\"",
        );
    });
}

#[test]
fn treat_bad_regex_as_a_string() {
    run(|t| async move {
        t.page
            .set_content("<a href=\"/foo\">Log in</a>")
            .await
            .unwrap();
        let m = message(
            expect(&t.page)
                .to_match_aria_snapshot("- link \"Log in\":\n  - /url: /[a/")
                .timeout(1)
                .await,
        );
        assert_contains(&m, "expect(page).toMatchAriaSnapshot(expected) failed");
        assert_contains(&m, "-   - /url: /[a/");
        assert_contains(&m, "+   - /url: /foo");
    });
}

#[test]
fn children_option() {
    // `toMatchAriaSnapshot.children` configuration.
    run(|t| async move {
        t.page
            .set_content("<ul><li>One</li><li>Two</li></ul>")
            .await
            .unwrap();
        let locator = t.page.locator("ul");
        expect(&locator)
            .to_match_aria_snapshot("- listitem: One")
            .await
            .unwrap();
        expect(&locator)
            .not()
            .to_match_aria_snapshot("- listitem: One")
            .children("equal")
            .timeout(1000)
            .await
            .unwrap();
    });
}
