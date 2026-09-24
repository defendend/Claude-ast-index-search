//! Which indexed paths hold tests.
//!
//! One definition for every command that tells tests from the code under
//! test: `explore` ranks test files below source, the symbol graph never
//! resolves production code to a definition inside a test tree, and
//! `graph --exclude-tests`, `hotspots --exclude-tests` and
//! `search --rank --exclude-tests` leave test files out.

/// Directory names that hold tests.
const TEST_DIRS: [&str; 4] = ["spec", "test", "tests", "__tests__"];

/// Languages whose test files are named after the class under test plus
/// `Test`, `Tests` or `Spec`: JUnit, Kotest, ScalaTest, Spock, XCTest,
/// NUnit / xUnit, PHPUnit, GoogleTest.
const CAMEL_CASE_TEST_LANGUAGES: [&str; 13] = [
    "java", "kt", "kts", "scala", "groovy", "swift", "m", "mm", "cs", "php", "cpp", "cc", "cxx",
];

/// Ruby sources and templates. Below `app/` and `lib/` every directory is a
/// module namespace of autoloaded code (`app/models/tests/` is
/// `module Tests`), and Ruby test frameworks use `spec/` and `test/`, never
/// `tests/` — which is also why an engine may be named `tests`.
const RUBY_LANGUAGES: [&str; 8] = [
    "rb", "rake", "ru", "erb", "haml", "slim", "jbuilder", "builder",
];

/// JavaScript and TypeScript, where a PascalCase directory is a component
/// (`components/Test/Test.tsx`) and test directories are lowercase.
const JS_LANGUAGES: [&str; 10] = [
    "js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts", "vue", "svelte",
];

/// Whether `path` (relative, `/`-separated) is a test file:
///
/// - its name follows a test convention: `*_test.*`, `*_spec.*`, `*.test.*`,
///   `*.spec.*`; `test_*.py` and `conftest.py`; `FooTest`, `FooTests`,
///   `FooSpec` in the languages that name tests that way
///   ([`CAMEL_CASE_TEST_LANGUAGES`]);
/// - or it sits in a directory named `spec`, `test`, `tests` or `__tests__`
///   (any letter case outside JavaScript / TypeScript), except a Ruby file
///   under `app/` or `lib/`, and a Ruby file in `tests/`
///   ([`RUBY_LANGUAGES`]).
///
/// `latest.rb`, `contest.py` and `Testimonial.kt` are not tests.
pub fn is_test_path(path: &str) -> bool {
    let (dirs, file) = path.rsplit_once('/').unwrap_or(("", path));
    let extension = file
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    is_test_file_name(file, &extension) || in_test_directory(dirs, &extension)
}

fn is_test_file_name(file: &str, extension: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    if ["_test.", "_spec.", ".test.", ".spec."]
        .iter()
        .any(|infix| lower.contains(infix))
    {
        return true;
    }
    if extension == "py" {
        return lower.starts_with("test_") || lower == "conftest.py";
    }
    if CAMEL_CASE_TEST_LANGUAGES.contains(&extension) {
        let stem = file.split('.').next().unwrap_or("");
        return ["Test", "Tests", "Spec"]
            .iter()
            .any(|suffix| stem.len() > suffix.len() && stem.ends_with(suffix));
    }
    false
}

fn in_test_directory(dirs: &str, extension: &str) -> bool {
    let ruby = RUBY_LANGUAGES.contains(&extension);
    let exact_case = JS_LANGUAGES.contains(&extension);
    for segment in dirs.split('/') {
        if ruby && (segment == "app" || segment == "lib") {
            return false;
        }
        let name = if exact_case {
            segment.to_string()
        } else {
            segment.to_ascii_lowercase()
        };
        if ruby && name == "tests" {
            continue;
        }
        if TEST_DIRS.contains(&name.as_str()) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::is_test_path;

    #[test]
    fn test_paths() {
        for path in [
            "spec/models/user_spec.rb",
            "spec/support/helpers.rb",
            "engines/billing/spec/services/charge_spec.rb",
            "test/test_helper.rb",
            "tests/integration.rs",
            "crates/core/tests/common/mod.rs",
            "src/components/__tests__/Button.tsx",
            "app/models/user_spec.rb",
            "pkg/server/handler_test.go",
            "pip/_internal/test_utils.py",
            "src/pkg/test_models.py",
            "src/conftest.py",
            "src/app.test.ts",
            "src/app.test.tsx",
            "src/app.spec.js",
            "app/src/main/java/com/acme/FooTest.kt",
            "Sources/App/SessionTests.swift",
            "src/main/scala/FooSpec.scala",
            "Tests/AppTests/Helpers.swift",
            "app/src/test/java/com/acme/FakeRepository.kt",
            "lib/matplotlib/tests/test_axes.py",
            "backend/app/tests/utils.py",
            "app/javascript/components/__tests__/card.js",
            "frontend/test/setup.ts",
            "src/Symfony/Component/Yaml/Tests/ParserTest.php",
        ] {
            assert!(is_test_path(path), "{path} is a test path");
        }
    }

    #[test]
    fn production_paths() {
        for path in [
            "app/models/latest.rb",
            "lib/latest.rb",
            "src/contest.py",
            "src/main/kotlin/Testimonial.kt",
            "src/main/kotlin/Test.kt",
            "src/testing.rs",
            "src/attest.go",
            // A domain named "tests" in a Rails application and its engine.
            "app/jobs/tests/grading/score_job.rb",
            "app/clients/exams/test/fetch_client.rb",
            "engines/tests/lib/tests/exam/grader.rb",
            "engines/tests/app/models/tests/exam.rb",
            "lib/tasks/test/seed.rake",
            // PascalCase component folders and names in JavaScript.
            "frontend/components/Test/Test.tsx",
            "frontend/components/StudentTests/StudentTests.jsx",
            "src/components/ContestSpec.ts",
        ] {
            assert!(!is_test_path(path), "{path} is not a test path");
        }
    }

    #[test]
    fn ruby_test_trees_count_before_app_or_lib() {
        assert!(is_test_path("spec/lib/importer_spec.rb"));
        assert!(is_test_path("spec/jobs/tests/score_job_spec_helper.rb"));
        assert!(is_test_path("engines/tests/spec/models/exam_spec.rb"));
        assert!(is_test_path("engines/tests/spec/factories/tests.rb"));
    }
}
