# Allow multiple browser instances

## What changes for the user

`open -na` launches independent Zephium processes. Shared startup state is coordinated, shortcut ownership transfers to a surviving instance, and Windows runtime generations stay isolated and recoverable.

## How I checked it

- Startup recovery and lease tests; full desktop suite.
- Mac development build and 16 distinct launch processes.
- Windows runtime tests compile; native Windows execution remains needed.
