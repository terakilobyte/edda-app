# EDDA release notes

Compiled into the app: the splash after an update shows the newest
section; Settings → Application → "What's new" shows them all. The
release script refuses to cut a version that has no section here.
Every section leads with a one-paragraph summary (the blurb the website
shows); everything after it is the full notes, folded behind "Full
notes" in the app and on the site alike.

## 0.3.0

EDDA now runs every search on the community API and downloads nothing
large: no galaxy or market data to install, no sync to wait for, and
no data-source choice to make. Trade finding returns round trips and
rings again, the Trade tab plans for any ship in your fleet (or one you
are about to buy), and the ship computer knows every synthesis recipe.

**One data source.** Route plotting beyond the bubble, trade finding,
market prices, station and system lookups and the fuel guard's star
field all come from the community API. A compact index of the populated
bubble ships inside EDDA for bubble-scale plotting. The data-source
choice and the System data page are gone; "Check for updates" lives on
the Ship computer settings page.

**Round trips are back on the API.** The server now runs the profit
finder's own pipeline, so round trips and rings come back with the
legs, and the Trade tab opens on round trips when the best loop
out-earns the best single leg. Supply and demand show under every leg.

**Plan for any ship.** The Trade tab's ship picker offers your fleet
(a stored ship starts from where it is stored) or "Other ship…" with
freeform cargo, jump range and pad. Origin, hold and range come from
the ship you pick.

**Synthesis and tech brokers.** Ask the ship computer for any synthesis
recipe (28 recipes, every grade, with how many you can make from what
is aboard) and for Guardian and Human tech-broker module costs.

**Grok.** xAI's Grok joins the ship computer's providers.

**Server-side.** Station pads, types and services for 640,000 stations
the server did not know; routes to systems the index has not learned
yet (a system EDDN carried an hour ago is routable now); the planner
pool and gates sized to the box, with long plots in their own lane.

## 0.2.9

A subtle bug prevented EDDA from operating properly and it has since been
addressed. EDDA also no longer is capable of running twice at the same
time, which could have caused consistency issues in settings in some
cases.

**One EDDA at a time.** Every version EDDA has ever shipped could be
started twice, and a second copy fought the first over the same database
and settings — which is why a setting could be chosen and then quietly
revert. Starting EDDA again now brings the window you already have to the
front instead of launching a rival.

**The what's-new window closes.** It was a full-screen panel that waited
for an answer from the app before it would go away, and when that answer
never came it sat over everything, swallowing every click. It closes
immediately now, and Escape works too.

**Market search works from the button.** Pressing Enter searched
correctly, but clicking Search sent the click itself to the server as if
it were a setting, and the search was refused.

**Quitting no longer freezes.** EDDA blocked its own window while waiting
for background work to stop, long enough for Windows to paint "Not
Responding" over the HUD. It now asks that work to stop first and waits
only briefly.

**Closing to the tray takes the HUD with it,** and opening EDDA from the
tray brings it back — unless you had hidden the HUD yourself, which is
remembered.

Fleet-carrier routing is hidden for now; it returns when it is properly
tested. Updates were rolled back to 0.2.5 while this was sorted out, and
this release supersedes that.

## 0.2.8

Updates actually arrive now: EDDA checks the moment it starts and every
few minutes after, instead of waiting two minutes and then six hours — so
a short session never missed the check entirely. "Check for updates" can
no longer hang: it gives up and tells you what went wrong. And installing
an update stops any data download already running, rather than writing to
the database while the app is being replaced.

## 0.2.7

Fixing our own stupidity.

## 0.2.6

Choose where your searches run — this machine or the community API — and
get a straight answer either way: when your own data has no coverage,
EDDA now says so instead of pretending there was nothing to find. Plus a
Mining page, a HUD you can size and fade, market boards you've visited
yourself, and a quieter voice on a trade run.

**Your data, your choice.** Onboarding and Settings now ask where
searches should run. **Use local data** keeps everything on this machine.
**Use the remote API** sends the search to the community server, which
holds the whole galaxy and answers in well under a second — no multi-
gigabyte download, and nothing about you is stored. Routes and trade
searches both honour the choice.

**Honest gaps.** When your local data has no coverage for a search — a
carrier whose board was never broadcast, a region your data doesn't
reach — EDDA now says exactly that and checks the community API for you,
instead of saying "nothing found", so the answer arrives here rather than
on some other site. The result is footnoted with where it came from, and
if the API can't help either it tells you that too rather than leaving
you guessing. Prefer to be asked first, or never? Data source in Settings
has both.

**Boards you have seen yourself.** Dock anywhere and that market becomes
searchable the moment the game writes it — including fleet carriers,
whose boards otherwise only reach EDDA if some other commander happens to
broadcast one. Your own eyes now count as a source, which is how it
should have worked all along.

**Mining.** A new page for finding what you mine: your own marked spots
first (private, stored only on this machine), then ring hotspots from the
community data, then the nearest bodies whose composition could carry it
— each labelled for what it is, a certainty or a probability. Materials
without a hotspot mechanic say so instead of returning nothing.

**Sell my hold.** One button prices everything in your cargo against the
boards around you and names the best place to take the lot.

**A HUD that fits.** New Settings → HUD tab: scale the overlay, fade the
background to anywhere between solid and fully transparent, and drag the
corner to resize the window itself.

**A quieter voice on a trade run.** Following a trade route no longer
draws fuel warnings and "docking at" chatter from the game's own route
narration — the router that planned the leg owns fuel, and the trade
follower names the pad you're actually going to. Legs are now marked
complete the moment you enter witchspace rather than on arrival, so the
HUD counts down when the jump happens. Targeting a system that isn't on
an EDDA-planned route now says so while you can still change your mind,
instead of waiting until you've arrived.

**Fixes.** The greeting and the status panel now name the ship you are
actually flying after a swap, both reading one source instead of
re-deriving it. Installs that hold only the recent community window now
show what data they have instead of showing nothing. The overlay's
transparency slider reaches 0%.

## 0.2.5

Follow a trade route the way you follow a plotted one, a sharper and
fresher trade finder, honest router distances, a calmer voice, and a
window that closes to the tray instead of quitting.

**Trade routes, followed.** Press ▶ on any result in the Profit finder —
a leg, a round trip, or a ring — and EDDA runs it with you: a TRADE
ROUTE block on the HUD, a spoken briefing at every pad (sell, then buy,
then the next system targeted automatically), lap counting with
measured-against-planned profit, and warnings when a board moved since
the search. Starting while docked at a stop just picks up from there.

**The trade finder got sharper.** Multi-stop rings are now opt-in (off
roughly halves a search), new minimum-supply and minimum-demand floors
hide thin boards, searches now default to prices from the last two hours
instead of two days so every result is current, the Out*/Back* columns
show each half of the loop with the arrival distance that drives it, and
every search reports where its time went.

**Routing honesty.** Your router-distance slider now applies everywhere
(short hops go to the game's own plotter, EDDA plans beyond it — and
the Settings label finally says so correctly), trade departures replan
at true laden mass, and the route page moved its warnings onto their
own row.

**A quieter, truer voice.** War-zone signals announce once with a count
("14 power conflict zones on sensors") instead of a burst; restarting
the app never replays history out loud; the speech server gets a
budget that scales with the line; your chosen voice and engine survive
restarts; and "full tank" can no longer be announced by a stale ship's
smaller tank.

**Ship computer follow-through.** Routes and trade searches asked by
voice now land in the Route and Trade tabs, and "clear my route"
clears the HUD immediately.

**Close to tray.** Closing the window keeps EDDA flying with you —
callouts, voice, overlay, live data — with a tray icon to bring it
back or quit for real.

**Under the hood.** Stations can no longer vanish from searches after a
patch day: journal files that Elite truncates and reuses are archived,
never deleted, and ghost system rows heal themselves.

## 0.2.4

One commodity, one row: the market catalog folded clean (search for any
good and find exactly one entry), plus quieter production logs.

## 0.2.3

Commodity canonicalization at every boundary, provisional 4.4.1.0 goods
metadata, and release plumbing that cannot lie about its version.

## 0.2.2

Anonymous telemetry (opt-out, allowlisted) with consent controls,
in-app problem reporting, honest data readouts, and complete ship
knowledge — Caspian Explorer and friends included.

## 0.2.1

First self-update delivered over the community API, sequential first
sync (voice, then routing, then market — no more memory pile-ups), map
polish, and stuck-target diagnosis.

## 0.2.0

EDDA meets the world: the community server at edda-app.com, signed
self-updates, the full first-sync experience, and the galaxy map with
Sol to the south.
