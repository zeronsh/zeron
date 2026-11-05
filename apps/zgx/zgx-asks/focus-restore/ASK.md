# Done upstream, waiting on a pin bump: Escape gives focus back

**Status:** fixed on zgx main in `f3b6b53e` ("Escape gives focus back as browsers do: to what had it, or nothing, and without a ring"). Zeron's pin `b3371f8` doesn't have it (`f3b6b53e` isn't its ancestor; it's in `fbf1d5e2` and `958be393` on origin/main). At `b3371f8`, `pop_focus_scope` still falls back to `next_focus` (input.rs:1670).

**On the bump:** un-ignore `focus_tests::escape_gives_focus_back_without_a_ring`; it's the acceptance check.
