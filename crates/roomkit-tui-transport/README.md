# roomkit-tui-transport

Bounded, ordered in-process request and event queues. Independent of the terminal,
engine and async runtime. Disconnecting a client does not cancel execution.

`connection(capacity)` returns the terminal's `Client` and the application's
`Host`. Sends never block: a full queue hands the value back so the caller can
retry. `Host::split` separates reading requests from publishing events, so a
host can read on a dedicated thread while other tasks publish through clones of
`HostEvents`.
