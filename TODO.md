# Future Sprints / Unimplemented UI Elements

The following items were mocked up in the UI redesign but remain non-functional and need backend wiring in future sprints:

- [ ] **Search & Filter Bar**: The text input for search does not actively filter the `tunnels` list.
- [ ] **Context Menu Options (⋮)**: The more options menu on each tunnel card.
  - **Planned**: Add RSA key management/configuration.
  - **Planned**: Options to view advanced logs or export config.
  - **Potential Options to Consider**:
    - View Advanced Logs (View real-time connection logs, debug info, and errors)
    - Export Configuration (Export to a JSON file)
    - Advanced SSH Options (Keep-Alive intervals, compression, timeouts)
    - Custom SSH Arguments (Add raw SSH arguments)
    - View Active Connections (Show currently active TCP connections routed through this tunnel)
- [x] **Live Telemetry & Diagnostics**:
  - Uptime Counter
  - Data Transfer (RX / TX) bandwidth calculation
  - Connection Quality Indicator (signal bars)
- [x] **Status Bar (Footer)**:
  - Dynamic Engine State (Connection Engine Active / Inactive)
  - Global Summary counts (e.g., Tunnels: 3 Active, 1 Stopped)
  - Compact / Tray Mode Toggle (icon on the right)

## Recently Completed
- **Visual Feedback**: Added press animations/visual feedback to the Floating Action Button (+), all Icon buttons (copy, edit, start/stop, delete), and Save buttons.
- **Left Navigation**: Wired up active state and switching for Settings and Tunnels tabs.
- **Floating Action Button (FAB)**: Wired to `create_new()` functionality.
