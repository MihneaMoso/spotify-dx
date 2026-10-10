package com.spotifydx.app;

import java.util.Arrays;
import java.util.List;

/**
 * Regression tests for the sheet/mini-player interaction layer added after
 * the provider-matching overhaul: drag follow + release thresholds, the
 * lyrics preview window + box height, the history highlight rebind, the
 * floating-chrome content inset, and the queue-count placement.
 *
 * Same hand-rolled harness as LogicRegressionTest (plain JVM, no JUnit):
 * {@code check()} calls driven from {@code main()}. Every case encodes a
 * real shipped bug or a user-specified number — the call sites delegate to
 * {@link SheetInteraction}, so these pin the behavior, not a copy of it.
 *
 * Deliberately NOT covered here (not JVM-reachable): theme-attr colors
 * (status-bar black pin, mini-player border, highlight wash), layout XML
 * structure, and animation timings — those stay device-verified.
 */
public class SheetInteractionTest {
    static int passed = 0;

    static void check(boolean cond, String name) {
        if (!cond) throw new AssertionError("FAILED: " + name);
        passed++;
    }

    static void checkFloat(float got, float want, String name) {
        if (Math.abs(got - want) > 0.001f)
            throw new AssertionError("FAILED: " + name + " got=" + got + " want=" + want);
        passed++;
    }

    public static void main(String[] args) {
        barDrag();
        sheetDrag();
        previewWindow();
        previewHeight();
        highlightRebinds();
        contentInset();
        queueCount();
        System.out.println("SheetInteractionTest: " + passed + " checks passed");
    }

    // -- Mini-player bar drag: 1:1 upward follow, quarter release ---------
    static void barDrag() {
        // Upward follows 1:1.
        checkFloat(SheetInteraction.INSTANCE.barFollow(0f, -200f, 2000f), -200f,
            "bar follows upward 1:1");
        checkFloat(SheetInteraction.INSTANCE.barFollow(-200f, -100f, 2000f), -300f,
            "bar accumulates upward");
        // Downward motion never sinks below rest.
        checkFloat(SheetInteraction.INSTANCE.barFollow(-200f, 500f, 2000f), -200f,
            "bar ignores downward");
        checkFloat(SheetInteraction.INSTANCE.barFollow(0f, 50f, 2000f), 0f,
            "bar stays at rest on downward");
        // Clamped to the screen top.
        checkFloat(SheetInteraction.INSTANCE.barFollow(-1900f, -500f, 2000f), -2000f,
            "bar clamps at screen top");
        // Release: past a quarter (rawY < 3/4H) opens.
        check(SheetInteraction.INSTANCE.barReleaseOpens(1499f, 2000f),
            "bar opens just past quarter");
        check(!SheetInteraction.INSTANCE.barReleaseOpens(1500f, 2000f),
            "bar stays exactly at quarter");
        check(!SheetInteraction.INSTANCE.barReleaseOpens(1999f, 2000f),
            "bar stays near rest");
        check(SheetInteraction.INSTANCE.barReleaseOpens(0f, 2000f),
            "bar opens at top");
    }

    // -- Sheet drag: 1.5x amplified follow, quarter release ---------------
    static void sheetDrag() {
        // Downward amplifies 1.5x (a flick covers real distance).
        checkFloat(SheetInteraction.INSTANCE.sheetFollow(0f, 100f), 150f,
            "sheet amplifies downward 1.5x");
        checkFloat(SheetInteraction.INSTANCE.sheetFollow(150f, 100f), 300f,
            "sheet accumulates amplified");
        // Upward bleeds back, never above rest.
        checkFloat(SheetInteraction.INSTANCE.sheetFollow(300f, -100f), 150f,
            "sheet bleeds upward at 1.5x");
        checkFloat(SheetInteraction.INSTANCE.sheetFollow(100f, -100f), 0f,
            "sheet floors at rest");
        // Release: past a quarter dismisses (strict >).
        check(!SheetInteraction.INSTANCE.sheetReleaseDismisses(500f, 2000f),
            "sheet stays exactly at quarter");
        check(SheetInteraction.INSTANCE.sheetReleaseDismisses(501f, 2000f),
            "sheet dismisses just past quarter");
        check(!SheetInteraction.INSTANCE.sheetReleaseDismisses(0f, 2000f),
            "sheet stays at rest");
    }

    // -- Lyrics preview window: prev2..next2, "" past the edges -----------
    static void previewWindow() {
        List<String> lines = Arrays.asList("a", "b", "c", "d", "e", "f", "g");
        check(SheetInteraction.INSTANCE.previewWindow(lines, 3)
                .equals(Arrays.asList("b", "c", "d", "e", "f")),
            "preview centers mid-list");
        // Head: missing past rows are empty, geometry held by minHeights.
        check(SheetInteraction.INSTANCE.previewWindow(lines, 0)
                .equals(Arrays.asList("", "", "a", "b", "c")),
            "preview pads head edges");
        check(SheetInteraction.INSTANCE.previewWindow(lines, 1)
                .equals(Arrays.asList("", "a", "b", "c", "d")),
            "preview pads single head edge");
        // Tail.
        check(SheetInteraction.INSTANCE.previewWindow(lines, 6)
                .equals(Arrays.asList("e", "f", "g", "", "")),
            "preview pads tail edges");
        // Empty lyrics: all blank, never crashes.
        check(SheetInteraction.INSTANCE.previewWindow(
                    java.util.Collections.<String>emptyList(), 0)
                .equals(Arrays.asList("", "", "", "", "")),
            "preview blanks on empty lyrics");
        check(SheetInteraction.INSTANCE.previewWindow(lines, 3).size() == 5,
            "preview always five rows");
    }

    // -- Preview box height: ~1/5 screen, clamped --------------------------
    static void previewHeight() {
        // Tall phone: 2400px @ 3.0 = 160dp exactly (cap).
        check(SheetInteraction.INSTANCE.previewBoxHeightDp(2400f, 3.0f) == 160,
            "preview caps at 160dp");
        // Very tall: clamped, never bloats.
        check(SheetInteraction.INSTANCE.previewBoxHeightDp(3200f, 2.0f) == 160,
            "preview clamps tall screens");
        // Small screen: floored so five rows still fit.
        check(SheetInteraction.INSTANCE.previewBoxHeightDp(800f, 2.0f) == 112,
            "preview floors at 112dp");
        // Mid phone lands inside the band unclamped: 2000/5/2.5 = 160.
        check(SheetInteraction.INSTANCE.previewBoxHeightDp(2000f, 2.5f) == 160,
            "preview mid phone hits cap");
        // A configuration landing mid-band passes through: 1500/5/2 = 150.
        check(SheetInteraction.INSTANCE.previewBoxHeightDp(1500f, 2.0f) == 150,
            "preview passes mid-band through");
    }

    // -- History highlight: rebind moved heads (DiffUtil never rebinds) ----
    static void highlightRebinds() {
        // Fresh song: old head's new slot + new head at 0.
        check(SheetInteraction.INSTANCE.highlightRebinds(
                    Arrays.asList("C", "B", "A"), "B").equals(Arrays.asList(1, 0)),
            "rebind covers old slot and new head");
        // Replay moved into place: its own slot + head.
        check(SheetInteraction.INSTANCE.highlightRebinds(
                    Arrays.asList("A", "C"), "C").equals(Arrays.asList(1, 0)),
            "rebind covers replayed head");
        // Trimmed-away old head: only the new head (holder recycled
        // through bind already).
        check(SheetInteraction.INSTANCE.highlightRebinds(
                    Arrays.asList("C"), "B").equals(Arrays.asList(0)),
            "rebind skips trimmed head");
        // No previous head (first load): just head.
        check(SheetInteraction.INSTANCE.highlightRebinds(
                    Arrays.asList("A"), null).equals(Arrays.asList(0)),
            "rebind handles first load");
    }

    // -- Floating-chrome content inset -------------------------------------
    static void contentInset() {
        check(SheetInteraction.INSTANCE.contentInset(true, 200, 150) == 350,
            "inset sums bar and nav");
        check(SheetInteraction.INSTANCE.contentInset(false, 200, 150) == 150,
            "inset drops hidden bar");
        check(SheetInteraction.INSTANCE.contentInset(false, 0, 0) == 0,
            "inset zero with no chrome");
        // Rail variant (no bottom nav): bar alone.
        check(SheetInteraction.INSTANCE.contentInset(true, 200, 0) == 200,
            "inset covers rail variant");
    }

    // -- Queue count lives in the top menu, queue tab only -----------------
    static void queueCount() {
        check(SheetInteraction.INSTANCE.queueCountVisible(true),
            "count shows on queue tab");
        check(!SheetInteraction.INSTANCE.queueCountVisible(false),
            "count hides elsewhere");
    }
}
