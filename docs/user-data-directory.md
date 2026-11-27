# Choose a user data directory

Append `--user-data-dir <path>` to the Zephium launch command to keep the
instance's application data in a chosen directory:

```sh
<Zephium executable> --user-data-dir /path/to/zephium-data
```

Relative paths are resolved from the current working directory. Zephium stores
its app data, cache and logs under the selected root. On Windows, supported
Work storage also uses this root and keeps its existing file protections.

Separate roots can run as separate instances at the same time. A launch using a
root that is already in use joins that instance. Without the option, Zephium
uses its current default data location.

The path must be usable and safely prepared by Zephium. New roots are private
on macOS and use a Windows ACL granting access to the current user, SYSTEM and
Administrators. Missing, empty, repeated or unsafe paths stop startup with an
error.
