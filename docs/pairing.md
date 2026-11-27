# First-time pairing

The case needs two separate pairings with your computer. One is for audio and one is for control.
Your desktop's Bluetooth settings make the first. `ozzzlo pair` makes the second.

| Pairing | Radio | Made by | Used for |
| --- | --- | --- | --- |
| Audio | Bluetooth Classic | Your desktop's Bluetooth settings | Streaming sound to the buds |
| Control | Bluetooth LE | `ozzzlo pair` | Every ozzzlo command |

Audio can work while ozzzlo reports "not bonded". That means only the first pairing exists.

## Before you start

- Close the Ozlo app on your phone. The case serves one control host at a time, and the phone app holds it while open.
- Put both buds in the case and leave the lid open.
- Find the button on the case. Holding it for 3 seconds puts the case in pairing mode.

Pairing mode lasts a few minutes and ends by itself. The case accepts new pairings of either kind only while it is on.

## Steps

1. Hold the case button for 3 seconds.
2. Pair the case for audio in your desktop's Bluetooth settings. It appears as "Ozlo Sleepbuds 2".
3. Run `ozzzlo pair`.
4. Do what it prints. It reports one of these, and prints a new line when the situation changes.

   | It prints | What to do |
   | --- | --- |
   | `Looking for the case. Keep its lid open.` | Wait. Holding the button drops the link for a moment, and ozzzlo retries. |
   | `The case is not in pairing mode. Hold its button for 3 seconds with the lid open.` | Hold the button. Pairing mode lapsed or was never on. |
   | `The case is in pairing mode. Pairing now.` | Nothing. |
   | `Paired. The case now accepts commands from this computer.` | Done. |
   | `Already paired. Nothing to do.` | Done. The control pairing already existed. |

5. Run `ozzzlo status` to confirm.

`ozzzlo pair` keeps trying for two minutes. You can run it before or after you hold the button.

## Keep audio connected

The case drops a host that holds only the control link. A few seconds after ozzzlo connects, the case announces that it is going idle and disconnects.
In the terminal interface this shows as the link dropping and returning every half minute.

Connect the computer to the buds for audio and the control link stays up.
Single commands such as `ozzzlo volume 35` finish before the case goes idle, so they work either way.

## When it goes wrong

| Message | Cause | Fix |
| --- | --- | --- |
| `this computer has never seen a Sleepbuds case` | The audio pairing does not exist, so ozzzlo cannot find the case. | Do steps 1 and 2. |
| `this computer is not bonded with the case. Run ozzzlo pair` | The control pairing is missing. Audio may still work. | Do steps 1, 3 and 4. |
| `the case did not pair in time` | Two minutes passed without the case in pairing mode and in reach. | Hold the button and run `ozzzlo pair` again. |
| `the case is not reachable over Bluetooth LE` | The phone app holds the control link, or the case is asleep or out of range. | Close the phone app. Open the lid. Try again. |
| `another program is already talking to the case` | Another ozzzlo is running, often the terminal interface. | Quit it first. |
| `the case went idle` in the terminal interface | No audio connection. | Connect audio, then press `c`. |

## Things to know

- Removing the case from your Bluetooth settings deletes both pairings. Start again from step 1.
- The phone app and ozzzlo cannot control the case at the same time. Whichever connects first keeps the control link until it lets go.
- ozzzlo asks the case to leave pairing mode once the control pairing exists.
- Pairing the computer did not stop the phone app from working in testing. That was checked on one phone.
- A computer that has never paired with the case for audio was not tested. In testing, the audio pairing was removed and made again on the same computer.

The log at `~/.local/state/ozzzlo/ozzzlo.log` records each pairing attempt. Run `ozzzlo -v pair` to watch it live.
