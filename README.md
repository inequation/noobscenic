# noobscenic

A clean-room, reverse-engineered replacement server for the **Proscenic M7 Pro**
robot vacuum, written in Rust.

The goal is to re-home the robot to a server you control — re-implementing the
vendor cloud's REST API, push gateway, and the local UDP config protocol — so it
keeps working without ever talking to Proscenic.

Protocol analysis and reverse-engineering artifacts live in [`doc/`](doc/).

Status: documentation only; no implementation yet.
