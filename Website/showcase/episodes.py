"""Episode titles and one-line descriptions for every fictional series in catalog.py.

TITLES[series id][season - 1] lists that season's episode titles. PLOTS[series id]
gives each episode slot its own line (season 1 uses PLOTS_S1 where written), so no
two episodes of a show, and no two shows, share a title or a description.
"""

TITLES = {
    "2001": [  # Harbor Lights
        ["The Tide Line", "Low Water", "What the Nets Bring", "Saint Brigid's Light", "Undertow", "The Inquest", "Fog Bell", "Harbor Lights"],
        ["Home Waters", "The Ferryman", "Salt", "A Name in the Register", "Slack Tide", "The Long Pier", "Breakwater", "Night Crossing"],
        ["Return", "The Quiet House", "Ballast", "Lantern Season", "Driftwood", "Two Keys", "The Last Boat Out", "Morning Light"],
    ],
    "2002": [  # The Meridian Files
        ["Pre-Filed", "Box 1147", "Signature", "Cold Storage", "The Reading Room", "Redacted", "Carbon Copy", "Appendix", "Margin Notes", "Index"],
        ["Unfiled", "The Other Archivist", "Chain of Custody", "Duplicates", "Overdue", "The Basement", "Microfiche", "Erratum", "Final Draft", "Received"],
    ],
    "2003": [  # Fieldwork
        ["Pilot Study", "Peer Review", "The Fern Situation", "Control Group", "Open Day", "Soil Samples", "The Inspector", "Funding Round", "Bloom", "Year One"],
        ["New Growth", "The Intern", "Hybrid", "Frost Warning", "The Rival Lab", "Pollination", "Sabbatical", "Abstract", "The Gala", "Germination"],
        ["Off Season", "The Greenhouse Effect", "Invasive Species", "Field Trip", "Seed Bank", "The Documentary", "Cross-Pollination", "Margin of Error", "Dormancy", "Harvest"],
        ["Fourth Season", "Repotting", "The Audit", "Root Systems", "Grafting", "The Award", "Wildflowers", "Late Frost", "Conference", "Perennial"],
    ],
    "2004": [  # Crown of Ash
        ["The King Is Dead", "Five Banners", "The Ashfield", "A Wedding at Thornmere", "The Vigil", "Ironwood", "The Burning Road", "Coronation"],
        ["Embers", "The Exiled Prince", "Winter Court", "Salt and Iron", "The Pale Queen", "Siege of Halloway", "What Remains", "Crown of Ash"],
    ],
    "2005": [  # Signal & Noise
        ["Launch Day", "Training Data", "Edge Cases", "The Whistle", "Ground Truth", "Overfit", "Dark Pattern", "Signal"],
    ],
    "2006": [  # The Living Coast
        ["Tidepools", "Kelp Forests", "Seabird Cliffs", "Mangroves", "Ice Shelves", "Islands"],
    ],
    "2007": [  # Low Orbit
        ["Grand Opening", "First Guests", "Dead Weight", "The Review", "Eclipse Season", "Docking", "Room Service", "Microgravity", "Burn", "Re-entry"],
        ["Second Launch", "The Honeymoon Suite", "Debris Field", "Inspection", "Solar Storm", "Zero-G Gala", "Supply Run", "The Investor", "Hull Breach", "Low Orbit"],
    ],
    "2008": [  # The Bureau of Lost Things
        ["The Blue Umbrella", "A Glove for the Left Hand", "The Wedding Ring", "Forty Keys", "The Violin Case", "A Letter Never Sent", "The Teddy Bear", "Unclaimed"],
    ],
    "2009": [  # Open Flame
        ["Kindling", "Smoke and Salt", "The Clay Oven", "Embers Only", "Breakfast at Dawn", "The Long Roast", "Family Recipes", "The Final Fire"],
        ["Back to the Hills", "Coal-Baked", "Fire and Ice", "The Market Challenge", "One Pot", "The Spit", "Desserts by Firelight", "Champion's Table"],
        ["Season of Smoke", "Wild Herbs", "Flatbreads", "The Rain Day", "Seafood on the Coals", "Feast for Forty", "Teams", "Last Ember"],
    ],
    "2010": [  # Tidewater
        ["Low Tide", "The Causeway", "Grandmother's Rules", "The Boathouse", "Summer Storm", "The Will", "High Water", "Ebb"],
        ["Flood Tide", "The New Owner", "Moorings", "Old Letters", "The Regatta", "Salt Marsh", "Spring Tide", "Tidewater"],
    ],
}

# Season-one lines written for each episode (other seasons use PLOTS below).
PLOTS_S1 = {
    "2001": ["Detective Nell Haskett comes home to Saint Brigid's Cove for a funeral and stays for an investigation.",
             "A missing trawler turns up at low water with its nets still full.",
             "The harbourmaster's ledger has one page torn out, and Nell knows whose handwriting is on the next.",
             "The lighthouse is lit for the first time in years, and nobody admits to climbing it.",
             "A storm traps Nell on the island with the one witness who saw everything.",
             "The inquest opens, and the town closes ranks.",
             "Nell follows the sound of the fog bell to a boathouse that was supposed to be empty.",
             "On the night of the lantern festival, the truth comes in with the tide."],
    "2002": ["Archivist June Calloway is handed a box that was filed six days from now.",
             "Every report in Box 1147 is accurate, and every one is signed by June.",
             "June compares her signature with the one on the files and finds one small difference.",
             "The cold storage vault holds the records of a department that doesn't exist yet.",
             "A reader requests a file that June hasn't written.",
             "Half of tomorrow's report has been blacked out, and June has the only pen.",
             "A copy of a report arrives before the original, and the original contradicts it.",
             "June finds an appendix describing the night she stops working at the archive.",
             "Notes in the margin of an old file are in a handwriting June is still learning.",
             "The archive's index lists one more box than the shelves can hold."],
}

# Per-series lines for every other episode slot, indexed by episode number.
PLOTS = {
    "2001": ["Nell takes the ferry to the mainland to find the man who sold the boat.",
             "An old case file reopens when the tide uncovers a car on the flats.",
             "The harbour board votes on the marina, and someone wants the vote delayed.",
             "Nell's father asks her for a favour she can't refuse.",
             "A diver finds something tied to the anchor chain.",
             "The pier is closed after dark, and a light is seen at the end of it.",
             "The sea wall gives way, and so does a long-kept alibi.",
             "Nell has one night to decide what the town is allowed to know."],
    "2002": ["A box arrives with no number, and the archive's records say it was never sent.",
             "June meets the archivist who will replace her, and recognises her coat.",
             "A file goes missing between two signatures.",
             "Two identical reports disagree about a single date.",
             "An overdue request turns out to have been made forty years early.",
             "The basement stacks are flooded, and only one shelf is dry.",
             "A reel of microfiche shows the reading room with June still in it.",
             "A correction notice is issued for an event that hasn't happened.",
             "June writes the last report, and has to decide whether to sign it.",
             "The archive receives a box addressed to the day after tomorrow."],
    "2003": ["Dr. Ines Marlow promises the board results by Friday, which is a problem.",
             "The station's only intern accidentally creates a plant that hums.",
             "A rare fern is discovered in the break room, and everyone wants credit.",
             "The control group is thriving and the experiment is not.",
             "Visitors arrive for open day, and the cactus collection is not ready.",
             "Theo labels every soil sample with the wrong year, beautifully.",
             "A ministry inspector loves the station a little too much.",
             "The team writes a grant application overnight, mostly in rhyme.",
             "Something in Greenhouse Two is blooming, and nobody planted it.",
             "The staff celebrate a year that nobody expected them to survive."],
    "2004": ["Princess Elayne is named regent for nine days, and nine days is a long time.",
             "The five houses meet under a truce that nobody intends to keep.",
             "A queen without a crown rides north to the ashfields.",
             "A marriage is arranged to end the war, and a second one to start it again.",
             "The old king's body lies in state, and one of his guards is missing.",
             "Lord Varric offers the exiled prince a sword and a price.",
             "The road through the burned valley is the only way home.",
             "The houses gather for a coronation, and only one of them is invited."],
    "2005": ["Maya Okafor's model goes live, and the first result surprises her.",
             "A colleague asks where the training data came from, and Maya can't say.",
             "Maya finds a pattern in the results that only appears in swing districts.",
             "A whistle-blower reaches out, and asks Maya to bring a copy.",
             "The company's lawyers want to talk to Maya about her search history.",
             "Maya rebuilds the model at home to prove what it's doing.",
             "A redesign of the company's app hides one setting from every user.",
             "Maya has one chance to make the world hear what she found."],
    "2006": ["Life in the rock pools, where every tide is a new world.",
             "Beneath the surface, forests of kelp shelter a city of creatures.",
             "A million seabirds return to the same cliffs every spring.",
             "Where the river meets the sea, the mangroves hold the coast together.",
             "At the edge of the ice, seals, bears and whales share a shrinking shore.",
             "On islands far from anywhere, evolution takes its own path."],
    "2007": ["The Halo Suites open for business, and the first guest is a critic.",
             "A celebrity couple wants the honeymoon suite, and the suite isn't finished.",
             "Manager Ade Mensah has to lose four hundred kilos before the next launch.",
             "The hotel's first review arrives, and it's from the guest who got stuck in the gym.",
             "Ninety minutes of darkness every orbit is hard on the staff, and the plants.",
             "A supply ship misses its docking window with the wedding cake on board.",
             "The chef discovers soup is a gas in microgravity, and so are the complaints.",
             "A guest goes missing in a station with nowhere to hide.",
             "An engine burn to dodge debris means everybody has to be strapped in for dinner.",
             "The crew comes down for a week and finds Earth very heavy."],
    "2008": ["A blue umbrella left on the night bus leads clerk Ada Penhallow across the city.",
             "A single glove has been handed in every winter for thirty years.",
             "Someone wants a wedding ring back, and someone else wants it lost.",
             "A ring of forty keys opens forty doors, and one of them is Ada's.",
             "A violin case comes in with no violin and a train ticket from 1962.",
             "Ada delivers a letter that was handed in still sealed.",
             "A child's teddy bear has been lost and found in four different countries.",
             "The bureau's oldest unclaimed object finally has a visitor."],
    "2009": ["Twelve cooks arrive in the hills and learn to build their own fires.",
             "Smoking, curing and salting: the cooks have until sundown.",
             "The clay oven is lit at dawn, and bread is due by noon.",
             "No flames allowed: everything is cooked in the embers.",
             "Breakfast for the whole valley, cooked over a single fire.",
             "A twelve-hour roast tests the cooks' patience more than their skill.",
             "The cooks bring the dish their families made over open fires.",
             "The finalists cook a three-course meal with nothing but fire."],
    "2010": ["The Calloway family gathers on the island for Grandmother Ruth's eightieth summer.",
             "The causeway floods, and the family is cut off for the night.",
             "Ruth lays down the rules for the summer, and her grandson breaks the first one.",
             "The old boathouse holds the family's best memories and a hidden debt.",
             "A summer storm brings the roof down and the truth out.",
             "Ruth's will is read early, at her insistence.",
             "The highest tide in a century reaches the porch steps.",
             "The family decides who will keep the house, and who will let it go."],
}

def title(series_id, season, number):
    seasons = TITLES[series_id]
    return seasons[(season - 1) % len(seasons)][(number - 1) % len(seasons[0])]

def plot(series_id, season, number):
    first = PLOTS_S1.get(series_id)
    if season == 1 and first and number <= len(first):
        return first[number - 1]
    pool = PLOTS[series_id]
    # Later seasons start further into the pool, so seasons don't mirror each other.
    return pool[(number - 1 + (season - 1) * 3) % len(pool)]
