---
name: mqtt-control
description: Control the LED matrix device over MQTT and run/monitor the local broker. Use when sending commands to the device (switch mode, set pixels/fill), testing the MQTT protocol, or starting the Mosquitto broker for the esp32-wasmi-led project.
---

# MQTT Control (esp32-wasmi-led)

The device connects over WiFi to an MQTT v5 broker, **subscribes to `host-esp32c6/mbox`** for commands,
and **publishes status to `test`**. Config lives in `host-esp32c6/src/mqtt.rs`.

## Broker

- Address: `192.168.1.201:1883` (hardcoded `BROKER_IP` in `host-esp32c6/src/mqtt.rs`), user `testUser`
  / pass `testPass`. The device's last will is published on `i/am/dead`.
- Run a local broker + monitor (Docker, from repo root):

```sh
just mosquitto          # Mosquitto on :1883 (config in mosquitto/config/mosquitto.conf, anonymous)
just mosquitto-monitor  # subscribe to all topics ('#') and print traffic
```

Notes:
- The local broker allows anonymous clients, but the device still sends `testUser`/`testPass`. To point
  the device at a different broker, update `BROKER_IP` (and creds) in `host-esp32c6/src/mqtt.rs` and rebuild.
- **You can start the broker yourself**, but `just mosquitto`'s recipe uses `docker run -it` (needs a
  TTY). For an unattended/background start, run it detached or without `-it`. The device only dials
  `192.168.1.201`, so the broker must run on that host (or change `BROKER_IP`).
- `mqtt_task` connects once at boot and **exits if the broker is unreachable** (`TCP connect failed` /
  `ConnectionReset`) — it won't retry until the device reboots, so have the broker up first.

## Command protocol

Publish JSON to `host-esp32c6/mbox`. `Command`/`DirectCommand` are externally-tagged serde enums
(`host-esp32c6/src/lib.rs`); `Point{x,y}` (top-left origin) and `Rgb{r,g,b}` are `u8` (0–15 for x/y,
0–255 for color).

```jsonc
{"SetMode":"Wasm"}                                                              // run guest WASM animations (default)
{"SetMode":"Direct"}                                                           // switch to direct pixel control
{"DirectCommand":{"SetPixel":{"point":{"x":5,"y":3},"color":{"r":255,"g":0,"b":0}}}}
{"DirectCommand":{"SetAll":{"color":{"r":0,"g":255,"b":0}}}}
```

`DirectCommand`s only take effect while the device is in `Direct` mode — send `{"SetMode":"Direct"}`
first.

## Examples (mosquitto_pub)

```sh
# switch to direct mode, then fill green
mosquitto_pub -h 192.168.1.201 -p 1883 -u testUser -P testPass -t host-esp32c6/mbox -m '{"SetMode":"Direct"}'
mosquitto_pub -h 192.168.1.201 -p 1883 -u testUser -P testPass -t host-esp32c6/mbox \
  -m '{"DirectCommand":{"SetAll":{"color":{"r":0,"g":255,"b":0}}}}'

# light a single red pixel at (5,3)
mosquitto_pub -h 192.168.1.201 -p 1883 -u testUser -P testPass -t host-esp32c6/mbox \
  -m '{"DirectCommand":{"SetPixel":{"point":{"x":5,"y":3},"color":{"r":255,"g":0,"b":0}}}}'

# back to animations
mosquitto_pub -h 192.168.1.201 -p 1883 -u testUser -P testPass -t host-esp32c6/mbox -m '{"SetMode":"Wasm"}'

# watch device status
mosquitto_sub -h 192.168.1.201 -p 1883 -u testUser -P testPass -t test -v
```

## Bulk / full-buffer updates

There is no batch command — `SetAll` fills one color, `SetPixel` sets one pixel. To paint an arbitrary
full frame (gradient, image, etc.) send all 256 `SetPixel`s. Stream them over **one** connection with
`mosquitto_pub -l` (publishes one message per stdin line) instead of 256 separate `mosquitto_pub` calls:

```sh
# Example: 4-corner bilinear gradient, white(TL) -> blue(TR) -> black(bottom).
# Top-left origin; x,y in 0..15. (TL=0,0  TR=15,0  BL=0,15  BR=15,15)
awk 'BEGIN{ for(y=0;y<16;y++) for(x=0;x<16;x++){
  u=x/15; v=y/15; g=int((1-v)*(1-u)*255+0.5); b=int((1-v)*255+0.5);
  printf "{\"DirectCommand\":{\"SetPixel\":{\"point\":{\"x\":%d,\"y\":%d},\"color\":{\"r\":%d,\"g\":%d,\"b\":%d}}}}\n", x,y,g,g,b
}}' | mosquitto_pub -l -h 192.168.1.201 -p 1883 -u testUser -P testPass -t host-esp32c6/mbox
```

Send `{"SetMode":"Direct"}` first. The host pixel buffer **persists between commands**, so each
`SetPixel` updates one pixel of the standing image and triggers a frame (it builds up incrementally);
you only need to set the pixels you want changed, or `SetAll` black first for a clean slate.

## Verifying

Malformed JSON is logged and ignored by the device (it won't crash). To confirm commands landed, grep
the `just run` log (the firmware streams over RTT) for `Parsed command:`, `SetPixel:` / `SetAll:` (these
are logged only once `direct_task` actually applies them, i.e. mode is `Direct` and the buffer pointer is
valid), or `Failed to parse:`.
