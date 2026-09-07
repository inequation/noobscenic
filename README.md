# noobscenic

A clean-room, reverse-engineered replacement server for the **Proscenic M7 Pro**
robot vacuum, written in Rust.

The goal is to re-home the robot to a server you control — re-implementing the
vendor cloud's REST API, push gateway, and the local UDP config protocol — so it
keeps working without ever talking to Proscenic.

Protocol analysis and reverse-engineering artifacts live in [`doc/`](doc/).

Status: phases 0-1 of [`doc/PLAN.md`](doc/PLAN.md) §15 are done — the re-home tool
works and the server runs, tracing and storing everything the robot sends, but it
does not answer the device's endpoints yet.
