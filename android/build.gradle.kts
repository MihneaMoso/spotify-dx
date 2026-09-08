// Owned Gradle project for the Kotlin Android app (docs/KOTLIN_MIGRATION.md
// §13). This replaces the dx-generated project: no generator step, the
// application ID is the real one (the template default leaked a placeholder),
// and the updater provider/sources are first-class source sets instead of a
// post-step staged by script.
//
// Versions are pinned to artifacts already in the Gradle cache so the build
// works fully offline (`--offline`): AGP 8.7.0, Kotlin Gradle Plugin 2.0.20,
// Gradle 9.1.0 (wrapper). If the AGP/KGP pair ever rejects the wrapper
// Gradle, pin the wrapper properties back to a proven pair instead of
// floating forward.

buildscript {
    repositories {
        google()
        mavenCentral()
    }
    dependencies {
        classpath("com.android.tools.build:gradle:8.7.0")
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:2.0.20")
    }
}

tasks.register("clean").configure {
    delete("build")
}
