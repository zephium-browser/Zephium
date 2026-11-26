---
name: trip-planning
description: Plan a trip end to end: where to stay, how to get there and back, entry rules, and a day-by-day plan with the total cost.
tools: [ask, start_part, create, finish]
---
# Trip planning

## First
- Know the dates (or length and month), where from, who travels and the budget. Ask only what the request and the thread leave open, in one ask with options (for a budget: "Under $2,000", "$2,000–4,000", "No limit"). Never ask what you can find out.
- A known event fixes the dates and the place (a YC batch runs at the YC office in San Francisco; check its dates with one web_search before asking).
- Dates not published yet (a batch "January to March", a conference "in spring") never stop the trip: ask once which dates to plan around, with the window as the first option ("Plan around Jan 4 – Mar 28, 2027") and "I'll give dates" as another. Once the person answers, or says to plan around the window, run Stay and Flights on those dates and label prices as for provisional dates. A plan without stays and flights is not a trip plan.

## Parts (start them in one turn, they run in parallel)
- **Stay** — browser part on airbnb.com (booking.com when the person prefers hotels), with `search` {place, checkin, checkout, adults, currency}: the helper starts on the site's results for those dates, so no date picker is needed (a month or more opens Airbnb's monthly stays). Brief: the area near where they need to be and the budget per night or per month. It returns three homes as picks with each home's own photos, the price as the results show it (per night or per month; a total only when the page shows one), rating, and the reason it fits. Homes read from the results are the part's result: it never opens homes or the site's front page to find totals.
- **Flights** — browser part on kayak.com (Google Flights as the second choice), with `search` {from, to, depart, return, adults, cabin, currency}, airports as IATA codes (WAW, SFO) and dates as YYYY-MM-DD; omit return for one way. Its records are one itinerary each, named by carriers and times ("LOT · 10:45 → 21:28, 1 stop"), never the page's sort tabs (Best, Cheapest). It returns three to four flights as picks with the route drawn (from, to, times, stops, carrier with its host such as lot.com), price and duration; mark the one you would book as recommended.
- Every search carries the currency of the person's budget, or of their own prices when the budget has none ("No limit" with a €800 fare): the code they wrote (EUR, CAD), the one currency a symbol names (€ EUR, £ GBP, zł PLN), or USD for a plain "$" when they name no other dollar (CA$, A$). Without one, leave it out. Kayak's address takes no currency, so its prices are recorded in the currency the page shows; nothing is converted.
- **Entry** — research part: visa or ESTA, passport validity, what to carry, for the traveller's nationality when known. It returns a short list (requirements), each item with its source.
- Add a part only for a real need the request names (a conference ticket, a car, a restaurant).
- One part per need: what Stay and Flights return is the result, provisional prices included. Never start a second part for the same flights or homes; a part that found nothing is retried from its row.

## Result
- One **plan** from the flight out to the flight back: travel steps for each flight (pick pointing at its flight pick), the stay (pick pointing at its home pick), using the picks marked recommended (to plan another, move recommended to it), the event days, entry steps before departure (kind task, with the deadline in when), and the flight home. Advice goes on the step it belongs to ("BART from SFO, about $10, 30 min"). Whenever the flight and the stay have prices in the same currency, the plan has a total: label "Estimated total", value the sum ("≈ $5,850"), flights + stay + known fixed costs. The stay counts for every night: a nightly price times the nights, a monthly price times the months (74 nights is about 2.5 months); its step's cost is that sum ("≈ $3,660") and its detail the basis ("$1,463/month × 2.5 months, before fees"). An estimate labelled as one, with its basis, is not a find dressed up: an unconfirmed checkout price is a reason to label it, never to leave it out. A home price with no unit is not a stay total; say what it is for. Prices in different currencies are never summed: each step keeps its own amount and currency, and the plan has no total.
- Then the **reply**: headline with the shape of the trip ("6 nights in SoMa, $3,420 in all" when the plan has a total, else "6 nights in SoMa"), text naming the recommended flight and home and the one thing to do first; figures for the estimated total when the plan has one, flight price and the stay's price with its unit (per night or per month), each in its own currency.
- Never make to-dos the agent could do itself ("Compare flights" is the Flights part's job). No guide document unless the person asked for one.
- A part that found nothing leaves its steps without a pick or price: no estimate from a search snippet stands in for a fare or a home, and the reply says in one sentence what is still open. Its row shows the fix.

## Follow-ups
- "Cheaper flights", "a different area", "book this one": run only the part it concerns, then revise that part's picks and the plan. Booking goes through the page and the app's Confirm; never say it is booked until the part reports it done.
