# Feat: Cannot select or create a named profile for one launch

There is no way to choose a named profile for one launch. `zephium --profile work` should open an existing `work` profile or create it on first use without changing the saved default. If creation or opening fails, Zephium should report the error and exit rather than open another profile.
