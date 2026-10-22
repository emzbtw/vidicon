#!/usr/bin/env python3
"""A fake Xtream provider with invented channels, for committable screenshots.

`demo_xtream.py <port> [channels]`: with a channel count, generated categories
are added after the screenshot set, for the performance pass (docs/perf/)."""
import base64, gzip, json, os, random, struct, subprocess, sys, tempfile, threading, time, zlib
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
from urllib.parse import urlparse, parse_qs, quote
from xml.sax.saxutils import escape

rng = random.Random(12)
now = int(time.time())

SPORT = ["Premier Darts Live", "Grand Prix Qualifying", "Test Cricket: Day 2", "Football Tonight",
         "Golf: Autumn Open", "Tennis: Harbour Masters", "Rugby Weekly", "Racing Replay",
         "Snooker: Northern Classic", "Cycling: Coast to Coast", "Boxing Night", "Sports Centre"]
GENERAL = ["The Morning Kitchen", "Village Vets", "Quiz Street", "The Long Road", "Evening Chat",
           "Home Rescue", "Coastal Lives", "Garden Hour", "Crime Files", "The Big Bake"]
NEWS = ["Evening News", "World Today", "Business Hour", "Politics Live", "Weather Update", "Newsroom"]
KIDS = ["Paperboat Pals", "Robo Ranch", "Mia's Map", "Tiny Trains", "Castle Club"]
DOCS = ["Wild Coasts", "Engines of Empire", "Deep Oceans", "Lost Cities", "Planet Weather", "Building Big"]
FILMS = ["The Last Harbour", "Midnight Signal", "Red Canyon", "Paper Kings", "The Quiet Year", "Glass City"]
MUSIC = ["Top 40 Countdown", "Live Sessions", "Classic Hits", "Late Night Mix"]

def cat(name, prefix, suffix, brands, titles, guide=0.75):
    return dict(name=name, chans=[(f"{prefix}{b}{suffix}", titles, guide) for b in brands])

CATS = [
    cat("4K| UHD ³⁸⁴⁰ᴾ", "4K: ", " ᵁᴴᴰ ³⁸⁴⁰ᴾ",
        ["NORTHWIND SPORT", "NORTHWIND SPORT+", "NORTHWIND ARENA", "NORTHWIND SPORT MAIN",
         "GRANITE RACING", "NORTHWIND SPORT 1", "COBALT CINEMA", "ATLAS NATURE", "ORBIT SCIENCE",
         "NORTHWIND SPORT DARTS", "NORTHWIND SPORT F1", "NORTHWIND SPORT GOLF", "KESTREL DRAMA",
         "TIDEWATER", "HARBOUR ONE", "MERIDIAN", "SOLSTICE TRAVEL", "EMBER FOOD", "LANTERN KIDS",
         "SABLE MUSIC"], SPORT),
    cat("UK| GENERAL ᴴᴰ/ᴿᴬᵂ", "UK: ", " ᴿᴬᵂ",
        ["HARBOUR ONE", "HARBOUR TWO", "HARBOUR THREE", "KESTREL", "KESTREL +1", "JUNIPER",
         "TIDEWATER", "SOLSTICE", "EMBER FOOD", "BRIGHTWATER"], GENERAL),
    cat("UK| SPORT ᴴᴰ", "UK: ", " ᴴᴰ",
        ["NORTHWIND SPORT MAIN EVENT", "NORTHWIND SPORT ACTION", "NORTHWIND SPORT CRICKET",
         "NORTHWIND SPORT FOOTBALL", "NORTHWIND SPORT TENNIS", "NORTHWIND SPORT RACING"], SPORT),
    cat("UK| NEWS ᴴᴰ", "UK: ", "", ["MERIDIAN NEWS", "HARBOUR NEWS", "ATLAS BUSINESS", "WORLDLINE"], NEWS),
    cat("UK| KIDS", "UK: ", "", ["LANTERN KIDS", "LANTERN JUNIOR", "PAPERBOAT", "TOPSPIN TOONS"], KIDS),
    cat("UK| DOCUMENTARY ᴴᴰ", "UK: ", " ᴴᴰ", ["ATLAS DOCS", "ATLAS NATURE", "ORBIT SCIENCE", "ORBIT HISTORY"], DOCS),
    cat("UK| CINEMA ᴴᴰ", "UK: ", " ᴴᴰ", ["COBALT PREMIERE", "COBALT ACTION", "COBALT COMEDY", "COBALT THRILLER"], FILMS),
]
BRANDS = ["SUMMIT", "REDWOOD", "BLUEFIN", "HALCYON", "VANTAGE", "PINNACLE",
          "CASCADE", "EVERGREEN", "LODESTAR", "MARINER", "QUARRY", "WILLOW"]
for region, label in [("US", "ENTERTAINMENT ⁶⁰ᶠᵖˢ"), ("US", "SPORT ⁶⁰ᶠᵖˢ"), ("US", "NEWS"),
                      ("CA", "GENERAL EN"), ("FR", "GÉNÉRAL HD/4K"), ("FR", "SPORT ⱽᴵᴾ"),
                      ("DE", "SPORT ᴴᴰ"), ("DE", "UNTERHALTUNG"), ("ES", "DEPORTES ᴿᴬᵂ"),
                      ("IT", "SPORT"), ("NL", "ALGEMEEN"), ("PT", "GENERALISTAS"), ("IE", "IRELAND HD/4K"),
                      ("SE", "NORDIC"), ("PL", "OGÓLNE"), ("AR", "SPORTS"), ("IN", "ENGLISH"),
                      ("AU", "GENERAL"), ("NZ", "SPORT"), ("MUSIC", "RADIO"), ("24/7", "CLASSICS")]:
    kind = SPORT if "SPORT" in label or "DEPORTES" in label else NEWS if label == "NEWS" else GENERAL
    brands = rng.sample(BRANDS, 6)
    CATS.append(cat(f"{region}| {label}", f"{region}: ", "", [f"{b} {i + 1}" for i, b in enumerate(brands)], kind, 0.6))
# The prototype's hard-coded recent and playing channels, by number.
CATS.append(dict(name="UK| NOW ᴴᴰ/ᴿᴬᵂ", chans=[], fixed=[
    (7042, "UK: HARBOUR ONE LONDON 4K ◉", GENERAL, 1),
    (8815, "NOW: NORTHWIND SPORT F1 ᴿᴬᵂ", SPORT, 1)]))
# Filed by format, not region (ticket 25): channel name rules hide the
# regions a category rule cannot.
CATS.append(cat("8K| UHD", "", " ⁸ᴷ",
    ["UK: HARBOUR ONE", "UK: NORTHWIND SPORT", "FR: SOLSTICE", "DE: GRANITE RACING",
     "PL: EMBER FOOD", "BE/FR: TIDEWATER", "KESTREL DRAMA | FR:", "AFR: SAVANNA ONE"], GENERAL))

PORT = int(sys.argv[1])
TOTAL = int(sys.argv[2]) if len(sys.argv) > 2 else 0

# Generated categories of up to 250 channels, until there are TOTAL.
left = TOTAL - sum(len(c["chans"]) + len(c.get("fixed", [])) for c in CATS)
KINDS = [("SPORT", SPORT), ("GENERAL", GENERAL), ("NEWS", NEWS), ("KIDS", KIDS),
         ("DOCUMENTARY", DOCS), ("CINEMA", FILMS), ("MUSIC", MUSIC)]
for k in range((max(left, 0) + 249) // 250):
    label, titles = KINDS[k % len(KINDS)]
    size = min(250, left - k * 250)
    # A third each tagged GEN:, untagged and tagged ALT|, so keeping the tag
    # gen (ticket 26) hides about a third, as on the real provider.
    prefix = ["GEN: ", "", "ALT| "][k % 3]
    CATS.append(cat(f"GEN| {label} {k + 1:03}", prefix, " ᴴᴰ" if k % 2 else "",
                    [f"{BRANDS[i % len(BRANDS)]} {label} {k + 1}-{i + 1}" for i in range(size)], titles, 0.6))

def icon(n, name):
    """Invented logos (ticket 13): most on a third-party host (`localhost`),
    every fifth on the provider's own host, some missing or broken."""
    brand = name.split(":", 1)[-1].split("|", 1)[-1].strip().split(" ")[0].lower()
    if TOTAL:
        # Each logo is shared by a few channels, as on large providers.
        brand = f"gen-{n // 4}"
    if n % 11 == 0:
        return ""
    if n % 13 == 0:
        return f"http://localhost:{PORT}/logo/missing-{n}.png"
    if n % 17 == 0:
        return f"http://localhost:{PORT}/logo/broken.png"
    host = "127.0.0.1" if n % 5 == 0 else "localhost"
    return f"http://{host}:{PORT}/logo/{brand}.png"

def png(w, h, px):
    raw = b"".join(b"\0" + bytes(c for x in range(w) for c in px(x, y)) for y in range(h))
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))

LOGOS = {}
def logo(brand):
    """A shape per brand: disc, white ring, wide bar or dark diamond."""
    if brand not in LOGOS:
        h = zlib.crc32(brand.encode())
        col = [(0xe0, 0x4f, 0x3f), (0x2f, 0x9e, 0x6d), (0x3f, 0x7f, 0xdf), (0xf2, 0xb1, 0x34)][h % 4]
        kind = (h >> 3) % 4
        if kind == 2:
            w, hh = 240, 96
            px = lambda x, y: (*col, 255) if (x // 24) % 2 == 0 else (255, 255, 255, 255)
        else:
            w = hh = 128
            def px(x, y):
                dx, dy = x - 64, y - 64
                d = (dx * dx + dy * dy) ** 0.5
                if kind == 0:
                    return (*col, 255) if d < 56 else (0, 0, 0, 0)
                if kind == 1:
                    return (255, 255, 255, 255) if 38 < d < 56 else (0, 0, 0, 0)
                return (20, 20, 24, 255) if abs(dx) + abs(dy) < 60 else (0, 0, 0, 0)
        LOGOS[brand] = png(w, hh, px)
    return LOGOS[brand]

def poster_url(kind, k):
    """Invented posters (VOD ticket 06), all on a third-party host
    (`localhost`) as on real providers; some missing, one not an image."""
    if k % 10 == 9:
        return ""
    if k % 23 == 22:
        return f"http://localhost:{PORT}/poster/missing-{kind}{k}.png"
    if k == 5:
        return f"http://localhost:{PORT}/poster/broken.png"
    return f"http://localhost:{PORT}/poster/{kind}{k}.png"

POSTERS = {}
def poster(name):
    """200 by 300: a gradient between two colours, and a sun or a horizon."""
    if name not in POSTERS:
        h = zlib.crc32(name.encode())
        cols = [(0x1d, 0x2b, 0x53), (0x7e, 0x25, 0x53), (0x00, 0x87, 0x51), (0xab, 0x52, 0x36),
                (0x5f, 0x57, 0x4f), (0x29, 0xad, 0xff), (0xff, 0x77, 0xa8), (0xff, 0xa3, 0x00)]
        top, bottom = cols[h % 8], cols[(h >> 3) % 8 or 1]
        sun = (h >> 6) % 2
        def px(x, y):
            if sun and (x - 100) ** 2 + (y - 120) ** 2 < 46 ** 2:
                return (0xff, 0xec, 0xd0, 255)
            if not sun and 190 < y < 204:
                return (0x14, 0x14, 0x18, 255)
            t = y / 300
            return (*(int(a + (b - a) * t) for a, b in zip(top, bottom)), 255)
        POSTERS[name] = png(200, 300, px)
    return POSTERS[name]

def archive_days(n):
    """Catch-up (SPEC 6.1): 3 days on every fourth channel of the screenshot
    set and on 1 in 50 after it, about as rare as on real providers; 7 days
    on channel 6."""
    return 7 if n == 6 else 3 if n % 4 == 1 and n <= 300 or n % 50 == 1 else 0

categories, streams, guide = [], [], []
# Past days `xmltv.php` does not reach, by stream id: only in the channel's
# `get_simple_data_table`, as on real providers (SPEC 6.1).
older = {}
# Each stream's titles, by stream id, for the external guides.
titles_by = {}
num = 0
fixed = {n for c in CATS for n, *_ in c.get("fixed", [])}
for ci, c in enumerate(CATS, 1):
    categories.append({"category_id": str(ci), "category_name": c["name"], "parent_id": 0})
    entries = []
    for name, titles, g in c["chans"]:
        num += 1
        while num in fixed:
            num += 1
        entries.append((num, name, titles, g))
    entries += c.get("fixed", [])
    for n, name, titles, g in entries:
        # SPEC 8.2 mockup: channel 2 has no guide, channel 6 has one.
        has = n == 6 or (n != 2 and rng.random() < g)
        epg = f"ch{n}.demo" if has else None
        days = archive_days(n)
        titles_by[1000 + n] = titles
        streams.append({"num": n, "name": name, "stream_id": 1000 + n, "category_id": str(ci),
                        "epg_channel_id": epg, "stream_icon": icon(n, name),
                        "tv_archive": int(days > 0), "tv_archive_duration": str(days)})
        if has:
            t = now - now % 1800 - rng.choice([1, 2, 3]) * 1800
            # A day back on a channel with an archive, as `xmltv.php` gives
            # (SPEC 6.1), from a generator of its own like the days ahead;
            # a day past the archive in its table.
            back, u = random.Random(-n), t
            while days and u > now - (days + 1) * 86400:
                d = back.choice([30, 60, 60, 90, 120]) * 60
                kept = guide if u > now - 86400 else older.setdefault(1000 + n, [])
                kept.append((epg, u - d, u, back.choice(titles)))
                u -= d
            while t < now + 12 * 3600:
                d = rng.choice([30, 60, 60, 90, 120]) * 60
                guide.append((epg, t, t + d, rng.choice(titles)))
                t += d
            # On to the guide's 3 days (SPEC 7.1) with a separate generator,
            # so the first 12 h, and every channel after, stay as they were.
            more = random.Random(n)
            while t < now + 3 * 86400:
                d = more.choice([30, 60, 60, 90, 120]) * 60
                guide.append((epg, t, t + d, more.choice(titles)))
                t += d

# Invented descriptions, one per title.
DESCS = [
    "Highlights, interviews and analysis from a busy week, with the stories behind the results.",
    "A relaxed look at the people and places of a small coastal town, told by those who live there.",
    "Two teams go head to head in a tense contest, with every point counting towards the final.",
    "The latest from home and abroad, followed by a look at what is coming up later in the day.",
    "An old friend returns with a secret, and the family has to decide who they can trust.",
    "Experts take on a difficult build against the clock, with a few surprises along the way.",
]
def desc(title):
    return DESCS[zlib.crc32(title.encode()) % len(DESCS)]

def xmltv():
    fmt = lambda t: time.strftime("%Y%m%d%H%M%S +0000", time.gmtime(t))
    out = ['<?xml version="1.0" encoding="UTF-8"?><tv>']
    for epg, a, b, title in guide:
        out.append(f'<programme start="{fmt(a)}" stop="{fmt(b)}" channel="{epg}"><title>{escape(title)}</title><desc>{escape(desc(title))}</desc></programme>')
    out.append("</tv>")
    return "\n".join(out)

# Two invented external guides (EPG sources ticket 02), for the screenshot
# set's channels. `/epg/match.xml` has the provider's own guide ids and
# titles of its own, so moving it above the provider's guide shows in the
# order. `/epg/other.xml.gz`, gzipped, covers the channels the provider has
# no guide for, by ids and names of its own (`Harbour Two (UK)`), so they
# match by the looser name form, and gives times an hour late, as a feed
# set in another zone would: `shift_minutes = -60` puts them right. Two of
# its channels share a name, so the channels with it are ambiguous.
def external(chans, late):
    fmt = lambda t: time.strftime("%Y%m%d%H%M%S +0000", time.gmtime(t + late))
    out = ['<?xml version="1.0" encoding="UTF-8"?><tv>']
    for cid, name, _ in chans:
        out.append(f'<channel id="{cid}"><display-name>{escape(name)}</display-name></channel>')
    for cid, _, titles in chans:
        if not titles:
            continue
        gen, t = random.Random(cid), now - now % 1800 - 3600
        while t < now + 36 * 3600:
            d = gen.choice([30, 60, 90]) * 60
            out.append(f'<programme start="{fmt(t)}" stop="{fmt(t + d)}" channel="{cid}"><title>{escape(gen.choice(titles))}</title></programme>')
            t += d
    out.append("</tv>")
    return "\n".join(out)

def external_feeds():
    shown = [s for s in streams if not categories[int(s["category_id"]) - 1]["category_name"].startswith("GEN|")]
    match = {s["epg_channel_id"]: (s["epg_channel_id"], s["name"], titles_by[s["stream_id"]])
             for s in shown if s["epg_channel_id"]}
    other = {}
    for s in shown:
        if s["epg_channel_id"]:
            continue
        region, _, rest = s["name"].rpartition(": ")
        brand = " ".join(w for w in rest.split() if w.isascii() and w not in ("|", "FR:")).title()
        region = region.split()[-1] if region else "INT"
        cid = f'{brand.lower().replace(" ", "-")}.{region.lower()}'
        other.setdefault(cid, (cid, f"{brand} ({region})", titles_by[s["stream_id"]]))
    first = next(iter(other.values()))
    other[first[0] + ".alt"] = (first[0] + ".alt", first[1], None)
    return external(list(match.values()), 0).encode(), gzip.compress(external(list(other.values()), 3600).encode())

# An invented M3U playlist of the same channels (M3U ticket 02), each
# streaming from `/live/` as the Xtream ones do, with the forms real lists
# use: CRLF, a duplicate `tvg-id` with a backup URL, one stream in two
# groups, a blank line and a comment inside an entry, unquoted attributes,
# a title with a comma, a long URL with a token, a URL with no `#EXTINF`,
# an `#EXTINF` with no URL (skipped and counted) and an
# `#EXT-X-CREDENTIALS` line, which is skipped.
def playlist():
    names = {c["category_id"]: c["category_name"] for c in categories}
    live = lambda sid: f"http://127.0.0.1:{PORT}/live/demo/demo/{sid}.ts"
    out = ["#EXTM3U", "#EXT-X-CREDENTIALS:none"]
    for i, s in enumerate(streams):
        group, title, url = names[s["category_id"]], s["name"], live(s["stream_id"])
        if i == 2:
            title += ", London"
        if i == 5:
            url += "?token=" + "".join(random.Random(5).choices("abcdef0123456789", k=1500))
        # Streams that need headers (M3U ticket 03): by option lines, and by
        # a Kodi-style `|` suffix. `/live/` refuses them without.
        if i == 6:
            url += "?need=" + quote("Demo/1.0 (X11, Linux)")
        if i == 7:
            url += "?need=" + quote("DemoPipe/2.0")
        if i % 7 == 3 and s["epg_channel_id"]:
            attrs = f'tvg-id={s["epg_channel_id"]} tvg-chno={s["num"]}'
        else:
            attrs = f'tvg-name="{s["name"]}" tvg-chno="{s["num"]}"'
            if s["epg_channel_id"]:
                attrs = f'tvg-id="{s["epg_channel_id"]}" ' + attrs
        attrs += f' tvg-logo="{s["stream_icon"]}" group-title="{group}"'
        out.append(f"#EXTINF:-1 {attrs},{title}")
        if i == 4:
            out += ["", "# the backup feed is below"]
        if i == 6:
            out += ["#EXTVLCOPT:http-user-agent=Demo/1.0 (X11, Linux)",
                    "#EXTVLCOPT:http-referrer=https://demo.example/"]
        out.append(url + ("|User-Agent=DemoPipe%2F2.0&Origin=https%3A%2F%2Fdemo.example" if i == 7 else ""))
        if i == 0:
            # A backup stream under the same `tvg-id`.
            out += [f"#EXTINF:-1 {attrs},{title} (backup)", url + "?backup=1"]
        if i == 1:
            # The same stream in a second group.
            out += [f'#EXTINF:-1 {attrs.replace(group, "Highlights")},{title}', url]
    out += [live(9999), "#EXTINF:-1,Gone"]
    return ("\r\n".join(out) + "\r\n").encode()

def table(stream_id):
    """`get_simple_data_table`: the channel's guide, older days included,
    titles and descriptions base64, `has_archive` on those its archive keeps."""
    s = next((s for s in streams if s["stream_id"] == stream_id), None)
    if not s or not s["epg_channel_id"]:
        return {"epg_listings": []}
    days, epg = int(s["tv_archive_duration"]), s["epg_channel_id"]
    b64 = lambda t: base64.b64encode(t.encode()).decode()
    progs = sorted(older.get(stream_id, []) + [g for g in guide if g[0] == epg], key=lambda g: g[1])
    return {"epg_listings": [{"id": str(i), "epg_id": "1", "title": b64(title), "lang": "en",
                              "description": b64(desc(title)), "channel_id": epg,
                              "start_timestamp": str(a), "stop_timestamp": str(b),
                              "now_playing": int(a <= now < b),
                              "has_archive": int(now - days * 86400 <= a and b <= now)}
                             for i, (_, a, b, title) in enumerate(progs)]}

# Movies and series (VOD ticket 02), invented. Category names begin as the
# real provider's do (SPEC 6.1): movies mostly `EN - `, series mostly with no
# prefix, so live category rules match none of them. Ids are numbered per
# kind, so series ids collide with live stream ids, as on real providers.
FIRST = ["The Last", "Midnight", "Red", "Paper", "The Quiet", "Glass", "Northern", "Silver",
         "Broken", "Hidden", "Winter", "Distant", "Golden", "The Long", "Electric", "Salt"]
SECOND = ["Harbour", "Signal", "Canyon", "Kings", "Year", "City", "Lights", "Road", "Tide",
          "Garden", "Engine", "Letters", "Orchard", "Frontier", "Summer", "Witness"]
MOVIE_CATS = ["EN - Drama", "EN - Comedy", "EN - Thriller", "EN - Family", "EN - Sci-Fi",
              "DE - Kino", "FR - Cinéma", "4K Films", "Documentaries"]
SERIES_CATS = ["Crime", "Comedy", "Drama", "Kids", "Documentary", "EN - Reality"]
GENRES = {"Crime": "Crime, Drama", "Comedy": "Comedy", "Drama": "Drama", "Kids": "Animation, Family",
          "Documentary": "Documentary", "EN - Reality": "Reality"}
PEOPLE = ["Ada Marsh", "Tom Reyes", "Lena Brook", "Sam Okafor", "Iris Vale", "Noah Pike",
          "Mia Stone", "Ravi Kent", "Ella Frost", "Jon Hale"]
PLOTS = ["A harbour town keeps a secret that a newcomer cannot leave alone.",
         "Two rivals are forced to work together when a storm cuts them off.",
         "A family inherits a crumbling hotel and the debts that come with it.",
         "An engineer finds a signal no one else can hear.",
         "Old friends reunite for one last summer before everything changes."]
vod = random.Random(7)

def title(k):
    return f"{FIRST[k % len(FIRST)]} {SECOND[(k * 7 + k // len(FIRST)) % len(SECOND)]}"

vod_categories = [{"category_id": str(i), "category_name": n, "parent_id": 0}
                  for i, n in enumerate(MOVIE_CATS, 1)]
movies = []
for c in vod_categories:
    for _ in range(vod.randint(8, 24)):
        k = len(movies)
        year = 1990 + k * 13 % 35
        movies.append({"num": k + 1, "name": f"{title(k)} ({year})", "stream_type": "movie",
                       "stream_id": 20000 + k * 17, "stream_icon": poster_url("m", k),
                       "rating": "" if k % 9 == 0 else f"{5 + k * 3 % 45 / 10:.1f}",
                       "rating_5based": round((5 + k * 3 % 45 / 10) / 2, 1),
                       "added": str(now - k * 86400), "is_adult": 0, "category_id": c["category_id"],
                       "category_ids": [int(c["category_id"])], "custom_sid": "", "direct_source": "",
                       "tmdb": "", "trailer": "",
                       "container_extension": ["mkv", "mkv", "mkv", "mp4"][k % 4]})
def vod_info(vod_id):
    """A movie's `get_vod_info` in the real provider's shape (SPEC 6.1):
    `duration` right, `duration_secs` in minutes as on the provider, cast
    and country empty on some."""
    m = next((m for m in movies if str(m["stream_id"]) == vod_id), None)
    if m is None:
        return {"info": [], "movie_data": []}
    k = m["num"] - 1
    cat = next(c["category_name"] for c in vod_categories if c["category_id"] == m["category_id"])
    secs = 80 * 60 + k * 431 % (70 * 60)
    year = m["name"][-5:-1]
    cast = "" if k % 7 == 0 else ", ".join(PEOPLE[(k + j) % len(PEOPLE)] for j in range(3))
    return {"info": {"kinopoisk_url": "", "tmdb_id": str(4000 + k), "name": m["name"][:-7],
                     "o_name": m["name"][:-7], "cover_big": "", "movie_image": "",
                     "releasedate": f"{year}-{1 + k % 12:02}-{1 + k % 28:02}",
                     "episode_run_time": str(secs // 60), "youtube_trailer": "",
                     "director": PEOPLE[k * 3 % len(PEOPLE)], "actors": cast, "cast": cast,
                     "description": PLOTS[k % len(PLOTS)], "plot": PLOTS[k % len(PLOTS)],
                     "age": "", "country": "", "genre": cat.split(" - ")[-1], "backdrop_path": [],
                     "duration_secs": secs // 60, "bitrate": 4000 + k,
                     "duration": f"{secs // 3600:02}:{secs // 60 % 60:02}:{secs % 60:02}",
                     "rating": m["rating"], "runtime": str(secs // 60), "status": "Released"},
            "movie_data": {k2: m[k2] for k2 in ("stream_id", "name", "added", "category_id",
                                                "category_ids", "container_extension",
                                                "custom_sid", "direct_source")}}

# Every movie streams the same short file: a test pattern, MOVIE_SECS long,
# made on first use, served with ranges so mpv knows its length and seeks.
MOVIE_SECS = 300
movie_file, movie_lock = os.path.join(tempfile.gettempdir(), f"vidicon-demo-movie-{MOVIE_SECS}.mkv"), threading.Lock()

def movie_path():
    with movie_lock:
        if not os.path.exists(movie_file):
            tmp = movie_file + ".part.mkv"
            subprocess.run(
                ["ffmpeg", "-loglevel", "error", "-y",
                 "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=25," + CLOCK.replace("text='", "text='MOVIE  "),
                 "-f", "lavfi", "-i", "sine=frequency=330", "-t", str(MOVIE_SECS),
                 "-c:v", "mpeg2video", "-q:v", "5", "-c:a", "mp2", tmp], check=True)
            os.replace(tmp, movie_file)
    return movie_file

series_categories = [{"category_id": str(i), "category_name": n, "parent_id": 0}
                     for i, n in enumerate(SERIES_CATS, 1)]
series = []
for c in series_categories:
    for _ in range(vod.randint(5, 14)):
        k = len(series)
        date = "" if k % 11 == 0 else f"{2000 + k * 7 % 25}-{1 + k % 12:02}-{1 + k % 28:02}"
        series.append({"num": k + 1, "name": title(k + 5), "series_id": 1001 + k,
                       "cover": poster_url("s", k), "plot": PLOTS[k % len(PLOTS)],
                       "cast": ", ".join(PEOPLE[(k + j) % len(PEOPLE)] for j in range(3)),
                       "director": PEOPLE[k * 3 % len(PEOPLE)], "genre": GENRES[c["category_name"]],
                       "releaseDate": date, "release_date": date, "last_modified": str(now - k * 3600),
                       "rating": f"{6 + k % 4}", "rating_5based": f"{(6 + k % 4) / 2}",
                       "backdrop_path": [], "youtube_trailer": "", "tmdb": "",
                       "episode_run_time": str([25, 30, 45, 60][k % 4]),
                       "category_id": c["category_id"], "category_ids": [int(c["category_id"])]})

EPISODES = ["Storm Warning", "Low Water", "The Night Ferry", "Below the Line", "The Long Tide",
            "Flares", "Harbour Wall", "Slack Water", "First Light", "Dead Reckoning", "Lantern",
            "Shout"]

def series_info(series_id):
    """A series' `get_series_info` in the real provider's shape (SPEC 6.1):
    `seasons` describes the show, so on every third it lists one more than
    `episodes` holds; episodes keyed by season, `id` a string (the stream
    id), `episode_num` a number or string, `title` the whole title, and an
    `info` whose own `id` is not the stream id. One in 11 has none."""
    s = next((s for s in series if str(s["series_id"]) == series_id), None)
    if s is None or s["num"] % 11 == 6:
        return {"seasons": [], "info": s or [], "episodes": []}
    k = s["num"] - 1
    year = int(s["release_date"][:4]) if s["release_date"] else 2015
    listed = 1 + k % 4
    have = listed - 1 if k % 3 == 0 and listed > 1 else listed
    run = int(s["episode_run_time"]) * 60
    episodes = {}
    for season in range(1, have + 1):
        eps = []
        for e in range(1, 7 + (k + season) % 7):
            secs = run - 120 + (k * 7 + season * 3 + e * 37) % 300
            eps.append({"id": str(50000 + k * 400 + season * 40 + e),
                        "episode_num": e if e % 2 else str(e),
                        "title": f"{s['name']} - S{season:02}E{e:02} - {EPISODES[(k + season * 5 + e) % len(EPISODES)]}",
                        "container_extension": "mkv" if (k + e) % 3 else "mp4",
                        "info": [] if (k + e) % 13 == 0 else {
                            "air_date": f"{year + season - 1}-{1 + e % 12:02}-{1 + k % 28:02}",
                            "duration_secs": secs, "bitrate": 3000 + e, "rating": 0,
                            "duration": f"{secs // 3600:02}:{secs // 60 % 60:02}:{secs % 60:02}",
                            "movie_image": "", "id": 900000 + k * 100 + e},
                        "custom_sid": "", "added": s["last_modified"], "season": season,
                        "direct_source": ""})
        episodes[str(season)] = eps
    return {"seasons": [{"air_date": f"{year + n - 1}-01-01", "episode_count": 8, "id": 7000 + n,
                         "name": f"Season {n}", "overview": "", "season_number": n,
                         "cover": "", "cover_big": ""} for n in range(1, listed + 1)],
            "info": s, "episodes": episodes}

ACCOUNT = {"user_info": {"username": "demo", "password": "demo", "auth": 1, "status": "Active",
                         "exp_date": str(now + int(float(os.environ.get("DEMO_EXPIRY_DAYS", "160")) * 86400)), "is_trial": "0", "active_cons": "1",
                         "created_at": str(now - 200 * 86400), "max_connections": os.environ.get("DEMO_CONNECTIONS", "2"),
                         "allowed_output_formats": ["m3u8", "ts"],
                         "message": "Welcome to Demo TV. Maintenance on Sunday, 03:00 to 04:00 UTC."},
           "server_info": {"url": "127.0.0.1", "port": str(PORT), "https_port": str(PORT + 1),
                           "rtmp_port": str(PORT + 2), "server_protocol": "http", "timezone": "UTC"}}

# The stream's own time on the pattern, to see where a recording starts.
CLOCK = r"drawtext=font=Inter:fontsize=72:fontcolor=white:box=1:boxcolor=black:x=20:y=20:text='%{pts\:hms}'"
# Seconds of stream sent at once on connect, as providers do (DEMO_BURST).
BURST = os.environ.get("DEMO_BURST", "0")
# Megabytes a second a movie or episode is sent at (DEMO_RATE), 0 for as
# fast as it goes: slow enough to watch a download run.
RATE = float(os.environ.get("DEMO_RATE", "0"))
# Seconds a live stream runs before the server closes it (DEMO_DROP), as a
# provider dropping it; 0 for never.
DROP = float(os.environ.get("DEMO_DROP", "0"))
# A live stream that goes silent from 20 s to 40 s of every minute, its
# picture black or a still slate (DEMO_BLACKOUT=black or slate), as a
# blackout would (stream recovery ticket 05).
OFF = "enable='between(mod(t,60),20,40)'"
BLACKOUT = {"black": f",drawbox=color=black:t=fill:{OFF}",
            "slate": f",drawbox=color=0x404060:t=fill:{OFF},drawtext=font=Inter:fontsize=40:"
                     f"fontcolor=white:x=(w-tw)/2:y=(h-th)/2:text='PROGRAMME UNAVAILABLE':{OFF}",
            }.get(os.environ.get("DEMO_BLACKOUT"), "")
# One request answered slowly or with an error, to show a refresh's
# stages (refresh panel ticket 01): `DEMO_SLOW=xmltv:20` sends the guide
# over 20 s, `DEMO_FAIL=get_vod_streams:503` answers the movie list with a
# 503. Keyed by `action`, or `xmltv` for `xmltv.php`; `epg` answers the
# external feeds with that status and `Retry-After: 60`.
SLOW = os.environ.get("DEMO_SLOW", "").partition(":")
FAIL = os.environ.get("DEMO_FAIL", "").partition(":")
# A live stream's timestamps run on from the server's start, as a broadcast's
# do, so a reconnect jumps ahead rather than starting again at 0.
START = time.monotonic()

EXTERNAL = external_feeds()
PLAYLIST = playlist()

class H(BaseHTTPRequestHandler):
    def do_GET(self):
        u = urlparse(self.path)
        action = parse_qs(u.query).get("action", ["account"])[0]
        if u.path.startswith("/logo/"):
            name = u.path[len("/logo/"):-len(".png")]
            if name.startswith("missing"):
                self.send_error(404)
                return
            body, ctype = (b"<html>not a logo</html>", "text/html") if name == "broken" else (logo(name), "image/png")
            self.send_response(200)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if u.path.startswith("/poster/"):
            name = u.path[len("/poster/"):-len(".png")]
            if name.startswith("missing"):
                self.send_error(404)
                return
            body, ctype = (b"<html>not a poster</html>", "text/html") if name == "broken" else (poster(name), "image/png")
            self.send_response(200)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if u.path in ("/epg/match.xml", "/epg/other.xml.gz"):
            if FAIL[0] == "epg":
                self.send_response(int(FAIL[2]))
                self.send_header("Retry-After", "60")
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            body = EXTERNAL[u.path == "/epg/other.xml.gz"]
            etag = f'"{zlib.crc32(body):08x}"'
            if self.headers.get("If-None-Match") == etag:
                self.send_response(304)
                self.send_header("ETag", etag)
                self.end_headers()
                return
            self.send_response(200)
            self.send_header("Content-Type", "application/xml" if u.path.endswith(".xml") else "application/gzip")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("ETag", etag)
            self.end_headers()
            self.wfile.write(body)
            return
        if u.path == "/playlist.m3u":
            if FAIL[0] == "playlist":
                self.send_error(int(FAIL[2]))
                return
            self.send_response(200)
            self.send_header("Content-Type", "audio/x-mpegurl")
            self.send_header("Content-Length", str(len(PLAYLIST)))
            self.end_headers()
            if SLOW[0] == "playlist":
                step = len(PLAYLIST) // 40 + 1
                for i in range(0, len(PLAYLIST), step):
                    self.wfile.write(PLAYLIST[i:i + step])
                    self.wfile.flush()
                    time.sleep(float(SLOW[2]) / 40)
                return
            self.wfile.write(PLAYLIST)
            return
        if u.path.startswith("/live/"):
            need = parse_qs(u.query).get("need", [None])[0]
            if need and self.headers.get("User-Agent") != need:
                self.send_error(403)
                return
            self.live(u.path.rsplit("/", 1)[-1].split(".")[0])
            return
        if u.path.startswith(("/movie/", "/series/")):
            self.movie()
            return
        if u.path.startswith("/timeshift/"):
            # /timeshift/{user}/{pass}/{minutes}/{YYYY-MM-DD:HH-MM}/{id}.ts
            parts = u.path.split("/")
            self.live(parts[-1].split(".")[0], minutes=int(parts[-3]), start=parts[-2])
            return
        # One invented account (onboarding ticket 01): others get `auth` 0.
        q = parse_qs(u.query)
        if (q.get("username", [""])[0], q.get("password", [""])[0]) != ("demo", "demo"):
            body = json.dumps({"user_info": {"auth": 0}}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        key = "xmltv" if u.path == "/xmltv.php" else action
        if FAIL[0] == key:
            self.send_error(int(FAIL[2]))
            return
        if u.path == "/xmltv.php":
            body, ctype = xmltv(), "application/xml"
        else:
            t = int(time.time())
            ACCOUNT["server_info"].update(timestamp_now=t, time_now=time.strftime("%Y-%m-%d %H:%M:%S", time.gmtime(t)))
            if action == "get_vod_info":
                body = json.dumps(vod_info(parse_qs(u.query).get("vod_id", [""])[0]))
            elif action == "get_series_info":
                body = json.dumps(series_info(parse_qs(u.query).get("series_id", [""])[0]))
            elif action == "get_simple_data_table":
                sid = parse_qs(u.query).get("stream_id", ["0"])[0]
                body = json.dumps(table(int(sid) if sid.isdigit() else 0))
            else:
                body = json.dumps({"account": ACCOUNT, "get_live_categories": categories,
                                   "get_live_streams": streams,
                                   "get_vod_categories": vod_categories, "get_vod_streams": movies,
                                   "get_series_categories": series_categories,
                                   "get_series": series}.get(action, []))
            ctype = "application/json"
        data = body.encode()
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        if SLOW[0] == key:
            step = len(data) // 40 + 1
            for i in range(0, len(data), step):
                self.wfile.write(data[i:i + step])
                self.wfile.flush()
                time.sleep(float(SLOW[2]) / 40)
            return
        self.wfile.write(data)
    def movie(self):
        """The movie file, whole or from a `bytes=N-` range; episodes too."""
        path = movie_path()
        size = os.path.getsize(path)
        r = self.headers.get("Range", "")
        start = int(r[6:].split("-")[0]) if r.startswith("bytes=") and r[6:].split("-")[0].isdigit() else 0
        self.send_response(206 if r else 200)
        self.send_header("Content-Type", "video/x-matroska")
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Length", str(size - start))
        if r:
            self.send_header("Content-Range", f"bytes {start}-{size - 1}/{size}")
        self.end_headers()
        try:
            with open(path, "rb") as f:
                f.seek(start)
                while chunk := f.read(65536):
                    self.wfile.write(chunk)
                    if RATE > 0:
                        time.sleep(len(chunk) / (RATE * 1e6))
        except (BrokenPipeError, ConnectionResetError):
            pass
    def live(self, stream_id, minutes=None, start=None):
        """A test pattern as an endless MPEG-TS, a tone pitched by channel.
        From the archive, `minutes` long and labelled with its `start`."""
        tone = 220 + int(stream_id or 0) % 40 * 20 if stream_id.isdigit() else 440
        clock, limit = CLOCK + BLACKOUT, ["-output_ts_offset", f"{time.monotonic() - START:.3f}"]
        mute = f",volume=volume=0:{OFF}" if BLACKOUT else ""
        if minutes is not None:
            label = start.replace(":", " ").replace("-", " ")
            clock, mute = CLOCK.replace("text='", f"text='ARCHIVE {label}  +"), ""
            limit = ["-t", str(minutes * 60)]
        ffmpeg = subprocess.Popen(
            ["ffmpeg", "-loglevel", "error", "-re", "-readrate_initial_burst", BURST,
             "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=25," + clock,
             "-f", "lavfi", "-i", f"sine=frequency={tone}" + mute, *limit,
             "-c:v", "mpeg2video", "-q:v", "5", "-c:a", "mp2", "-f", "mpegts", "pipe:1"],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE)
        self.send_response(200)
        self.send_header("Content-Type", "video/mp2t")
        self.end_headers()
        drop = time.monotonic() + DROP if DROP and minutes is None else None
        try:
            while chunk := ffmpeg.stdout.read1(65536):
                self.wfile.write(chunk)
                if drop and time.monotonic() > drop:
                    break
        except (BrokenPipeError, ConnectionResetError):
            pass
        finally:
            ffmpeg.kill()
            ffmpeg.wait()
    def log_message(self, *a):
        pass

srv = ThreadingHTTPServer(("127.0.0.1", PORT), H)
print(f"{len(categories)} categories, {len(streams)} channels, {len(guide)} programmes, "
      f"{len(movies)} movies, {len(series)} series", flush=True)
srv.serve_forever()
