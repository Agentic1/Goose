# Consumer Group Enhancements (August Progress)

## Recent Improvements
- Envelope now tracks `consumer_group`, `consumer_id`, and `delivery_count` fields
- Bus library exposes helpers to create groups, read using `XREADGROUP`, and acknowledge messages
- `delegate_with_opts` and `web` command updated to use consumer groups for request/response handling

## TODOs
- Add unit tests for new helper methods in `bus` crate
- Integration tests for CLI and meta delegation using consumer groups
- Monitor pending messages and retry handling
- Document new bus APIs in README
