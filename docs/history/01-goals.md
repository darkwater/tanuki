# Goals and examples

## Requirements

- Smart home is a primary use case, alongside broader personal automation.
- Provide a central fabric for scripts, integrations, and user interfaces to exchange data.
- Allow arbitrary values at arbitrary names without mandatory device modelling.
- Support publishing a value now and retrieving it later from elsewhere.
- Make client integration easy across desktops, Linux systems, smartphones, and small scripts.
- Plan for MQTT and HTTP entrypoints, with other protocols possible. They need not all ship in the first implementation.
- Do not require a separate, fully specification-compliant MQTT broker. MQTT is an interface to the system; the exact supported subset remains open.
- Provide basic central logging and error visibility while retaining room for quick, informal integrations.

## Motivation

A previous useful but fragile system stored arbitrary values in Redis, probably behind an HTTP API. Portable devices submitted battery levels through cron jobs, Tasker, or other automation. Widgets could retrieve all reported battery levels.

Another smart-home-oriented attempt used MQTT topic conventions for entities and capabilities such as on/off and colour. Its modelling coupled one connection to one device, including availability through a will topic, and made composite devices awkward. The new design should avoid that coupling.

## Example workloads

| Data | Use |
| --- | --- |
| Estimated position within one room, derived from motion sensors | Turn on the kitchen-area light when entering that area |
| TV power and HDMI source | Select content for an OLED tablet below the TV using this and other variables |
| Custom voice assistant status, such as listening | Display an indicator on a TV, tablet, or ceiling lamps |
| Desktop applications, clipboard, and workspace | Cross-process automation and displays |
| Battery state | Show several devices' batteries in a shared widget |

## Current directions

- Optional schemas governing namespaces, for example the shape of values beneath `/battery/`.
- Schema violations should surface as errors.
- Potentially run some scripts inside the system using JavaScript, WASM, or another runtime.
- Expose runtime script health as data, for example `/runtime/scripts/<script>` with a status field.
- Allow a script to consume log entries and forward notifications to Telegram.

The user accepts that a notification script failing could prevent notification of that failure. This is a personal system with basic robustness expectations, not a requirement for redundant monitoring infrastructure.
