# EDDA release notes

Compiled into the app: the splash after an update shows the newest
section; Settings → Application → "What's new" shows them all. The
release script refuses to cut a version that has no section here.
Every section leads with a one-paragraph summary (the blurb the website
shows); everything after it is the full notes, folded behind "Full
notes" in the app and on the site alike.

## 0.3.3

A veteran's journal reads correctly again: EDDA no longer shows a 2022
ship and "still in flight" to a commander who played through the game's
2022 journal rename. Mute and a hidden HUD survive a restart. Material
traders come back typed as raw, manufactured and encoded. A Coriolis
paste on the route page says what it lacks and where to type the range.
A dependency security advisory is fixed and the release now checks for
them.

**Your journal in the right order.** In late 2022 the game changed how
it names journal files, and the two names do not sort together as
plain text: 2021 and 2022 files land after 2026. EDDA trusted that
order, so a commander with years of history saw their 2022 ship as
current, had materials rebuilt with 2022 changes applied after today's,
and could have lost live callouts. Every read that means "in time
order" now goes by the events' own timestamps, and both file listings
sort the two name formats on one clock.

**Mute means mute.** "Mute all callouts" only lasted until the next
launch, and the greeting spoke to commanders who had asked for silence.
It is remembered now, and applied before anything can speak. A hidden
HUD is remembered the same way, whether hidden with Ctrl+Shift+H or from
Settings.

**Material traders by kind.** The community data now carries each
station's economy, so the Engineering tab's nearest raw, manufactured
and encoded traders are typed again instead of "kind unknown". When the
community API cannot be reached, EDDA says so rather than reporting an
empty galaxy.

**Route page and Coriolis.** Coriolis's export carries the ship and its
modules but not the mass, tank or range the physics needs. The page now
says exactly that, names the missing numbers, and puts the cursor in the
jump-range box so the plot can still run. EDSY's export works as before.

**Under the hood.** rustls updated for RUSTSEC-2026-0285, and the
release itself now runs the licence and advisory check that only CI ran
before. The server reads station rows by name rather than position, the
change that briefly took station lookups down after 0.3.2, and its
database integration tests now run in CI.

## 0.3.2

The Engineering tab is back, and the searches that quietly returned
nothing now work: ships and modules by the names you call them, material
traders, and every station service rather than three. Ship and module
discounts show where they apply. Carrier callouts stopped confusing your
carrier with the squadron's, and the ship computer hands short routes to
the game's own plotter instead of planning them itself.

**Search by the names you use.** The Market tab matched what you typed
against the game's internal symbols, so a search only worked when the
two happened to agree: "Mandalay" found fifty stations, "Type-10
Defender" found none, and neither did "Imperial Cutter", "Krait Mk II",
"5A fuel scoop" or "beam laser". EDDA now translates before it asks —
display names, internal symbols, or a unique fragment like "defender" —
and the ship search completes as you type, offline.

**Discounts.** Every ship and module row shows the discount that applies
there and why, from the game's published rules: Li Yong-Rui's space, the
weapon discounts in Jerome Archer's, Mahon's cargo racks and hull
reinforcement, Patreus's Imperial hulls, the permit stations such as
Jameson Memorial, and your own Elite rank, which stacks. "Discounted
only" narrows the search to where a discount exists rather than filtering
the nearest results and leaving you nothing. Fleet carriers and a Power's
Stronghold Carriers are now separate switches.

**The Engineering tab returns**, with blueprint costs, your materials,
engineer access and the trader shopping list — and the Plan button on
every module in the Ships tab that hands it straight there.

**Materials you hold are counted.** "Untypical Shield Scans" read zero
against a hold of 131: one stray space in EDDA's own material table filed
them under a name no blueprint spells. Fixed, with tests that keep every
material findable by the name it shows.

**Material traders appear again.** They are typed by their station's
economy, which the community data does not publish yet, so EDDA had been
discarding every trader and reporting none. Until that lands it lists
every material trader in range and says the kind is unknown, and it looks
300 light-years out instead of 150.

**Services search.** The Galaxy tab offered three services; it now offers
all of them, interstellar factors included, taken from the same
vocabulary the data uses so the two cannot drift. Leave the origin empty
to search from where you are.

**Ships and modules across your whole fleet.** Ask which of your ships
carries a wake scanner and the ship computer searches every owned ship's
stored build, not just the one you are flying, and can show any of their
builds.

**Carriers.** An undocked carrier jump and a heartbeat after departure
both clear a pending jump, so a scheduled jump no longer sticks at
"departs in -1350 min". Your carrier and a squadron's are reported
separately, each remembering where it is.

**Routing.** A route within your route-coverage threshold now goes to the
game's own plotter when you ask the ship computer for it, the way the
Route tab and trade following already did. A re-plan that produces the
plan you are already flying no longer announces itself. A route that
cannot exist is refused in seconds instead of minutes.

## 0.3.1

Linux: the AppImage starts again on newer distributions. Everything
else is 0.3.0.

**AppImage on newer glibc and glib.** The 0.3.0 AppImage bundled the
build machine's `libmount` and `libpcre2` next to a glib taken from
the host, and a host with a newer glib refused to start
(`version 'MOUNT_2_40' not found`). glib's whole family now comes from
the host, the way glib itself already did. The `.deb` was never
affected. What EDDA needs on Linux is written down at
edda-app.com/linux.

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
