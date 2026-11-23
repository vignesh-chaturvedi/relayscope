# Kill test, 2026-11-23 12:26:57 UTC

Relayscope followed the Donation Relay program through the websocket source at `confirmed` commitment. The source was cut at slot 472934969 and held down for 1427 seconds (at least 300, and until transactions had landed during the outage). After it came back, every signature RPC lists for the program between slots 472934819 and 472941741, at `finalized` commitment, was looked up in the store.

| | |
| --- | --- |
| Result | pass: nothing missing |
| Outage | slots 472934969 to 472941591, 1427 seconds |
| Gap filled by | the repair pass, which listed the gap from RPC and fetched what was missing |
| Window checked | slots 472934819 to 472941741 |
| Listed by RPC | 6 signatures, 5 of them during the outage |
| Stored | 1 through the websocket source, 5 by the repair pass |
| Missing from the store | 0 |
| Announced but not listed by RPC | 0 |
| Delay from block to store, through the source | median 1 seconds, longest 1 seconds, over 3 transactions |

## During the outage

- slot 472941521: `2NxY8288UQnqUCe1kKiZ78EpsoT3TXAinGCitehgfLyhm8THVjvoAtWfMoJbo3hQevQ5JJDBC5h1KdZX4AAz9HjE`, stored by the repair pass
- slot 472941512: `4pN9MNG1SJ8Buz6gQuF9Yh4SJNX532Dj4kh2yz5wwnGE65rBfiLhu1AVESPfbXfwBqbeQby1u6VgAyvgHKXNe88Z`, stored by the repair pass
- slot 472941493: `vnFVENVavkewvwYcLgniBqH9MxK97mCeXP9RxDJcAWAstANokeVEUHjwA4ij8J1uWLMzNHvVftNjrLPQ1pwSi5r`, stored by the repair pass
- slot 472941468: `3h2r4qRnbhc6yNFg6TNR2qRFU4EnzsDrYeTuSueZ5rmyQkUsvhANC1iEmje5i3g2kLKJQHxnvcN61yy7TDQbopHR`, stored by the repair pass
- slot 472934991: `3CALCP5t16MinsNSJmSUx6Pd94qLeT29YJSkPayVPe1nykJxooGtZzhDsWLW9jPwUpdGuzZRnAkzWwQHQoSJpEhR`, stored by the repair pass
