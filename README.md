# Milestone 4: Web Server + Lobbies

In this assignment, you will build a web-based authentication system for the
video game, and integrate it with your milestone 3 solution.

While working on this assignment, you will:

1. Do backend web development and database programming.
2. Handle different authentication schemes (password, tokens).
3. Use coding agents (optional).

You need to _pass_ this assignment in order to qualify for an A- or above.

## Deadline: May 19

- This is the last day for you to **pass all code reviews** and schedule an
  interactive grading session.
- I will have less availability for interactive grading during finals week.

## Setup and testing

You need to figure these out, and writing tests is a general concern for this
assignment that you need to address.

## Directory structure

Same as milestone 2.

```
.
├── Cargo.lock
├── Cargo.toml
├── CODEOWNERS
├── deploy-client.sh   -- deploys the client to static/
├── README.md
├── run-game-server.sh -- runs a game server, you shouldn't need this except for some local debugging.
├── run-web-server.sh  -- runs YOUR web server
├── static             -- where static content (e.g. the wasm files) should go
│   └── index.html     -- included as an example page that wraps the wasm file
├── templates/         -- where the templates should go, contains an empty file for Git
└── web-server/        -- this is just an empty Rust project with the right dependencies
```

## Starting the assignment

You should copy the `server`, `common`, `client` directories from milestone 3,
and register them in `Cargo.toml`.  Then, you should be ready to do the
assignment.

After you check in this code, request a code review from me, and I will approve
it ASAP to let you move on to the actual code reviews.

## CI

There is no CI setup for this assignment, building a CI counts as a feature.

## What you need to implement

Features 1--2 are required.  You get to choose among other features (see the
grading section).

You should create **one PR per feature**.

### Feature 1: Add a user system

As a starting point for this, you can use the `sessions` example with users +
counters from the lectures.

You need to:

- Create a database schema that involves users, and games they were in with the
  verdict of each game.
- Have password-based auth backed by a SQLite database.
- Create login/signup/user list/user detail/currently online pages with proper
  backend support.
- Pre-populate this with some fake data (you can use AI coding agents to
  generate fixtures).
- Write some basic tests.

Interactive grading demo:

- Create a new user and log in.
- Show user list and user stats pages for an existing user.
- Show different failure modes for authorization.

### Feature 2: Lobbies and spawning game servers

Add a lobby system **in the web server** with the following:

- Let a user create and/or join a lobby if they aren't already in a lobby.
- Lobbies should have size (1--3 people).
- Once the lobby is created, a corresponding game server needs to spawn.
- Once the game ends or the last player leaves (in case the game hasn't
  started), **the game server** should delete the lobby, save the outcome to the
  game record.

Interactive grading demo:

- Create two lobbies of size 1 and start two separate games, show port numbers
  in the client.
- Create a lobby of size 2 and join as a separate user, show that the 2-player
  game works.
- Show different failure modes (e.g., that someone cannot create a lobby without
  logging in).
- Try joining two lobbies simultanously from the same account.
- Try joining a lobby after another game finished in a different lobby, using
  the same account for both actions.
- Show that a lobby (and the game server) is killed after a certain period of
  inactivity (for the demo, set this to one minute).

### Feature 3: Authentication

- Switch the game server and the client to use netcode.
- When a user creates/joins a lobby, create an auth token and send it to the
  user.  They should immediately join the game server.
- After a game ends, the game server should record the outcome of the game in
  the database.
  
Interactive grading demo:

- Show and explain how you switched to netcode.
- Show the game records work.

The expected workflow for auth here is:

- Creating a lobby starts a game server with a shared fresh private key.
  - The game server should be spawned as a separate process that runs in the
    background.
  - There needs to be enough coordination between the servers to know which game
    this server belongs to so it can record the game outcome.
- Each user receives a **unique** connect token based on a unique user ID.
- The Rust client fetches the token from DOM or JavaScript.
- The client then connects to the game server.
- Once all clients are connected, the game starts.

### Feature 4: Spectators and replay

- Allow spectators to join a live game and observe others.
- Adapt the record/replay code from previous milestones to record games, and
  play them back directly on the client (or by spawning a 1-player game server,
  up to you).
  - You should store the recorded games in a directory rather than the database
    as they are very large blobs.
  
Interactive grading demo:

- Start a 1-player game and join it as a spectator.
- Replay a 2-player game.
- Investigate and talk about different options for storing/serving game
  recordings and what you can do to make them as small as possible.
  
## Feature 5: Set up the CI

- You can base it off of milestone 3 CI config.
- Show that the CI works and runs your tests.

If you go this route, you need to do feature 5 before feature 2.
  
## AI use policy

You are free to use AI agents however you want for this project.  However, **you
are responsible for the code you submit, and your knowledge of it**.  If you
cannot explain the code and the ideas behind it well enough that you can easily
manipulate them during code reviews, interactive grading, or the exam you will
fail this assignment.

## Grading

You need to implement and pass the code review reviews for 3 out of 5 features
to qualify for interactive grading.  Features 1 and 2 are required, and you get
to choose between features 3, 4, 5.

### Code review

You need to request code reviews for each major feature you implement.  Unlike
previous milestones, code reviews will be more focused on overall structure of
the code and less about small details.

The hard deadline for this assignment is the deadline to
**get approval and merge all your PRs**.

After you are assigned a reviewer, they will review your code and request
explanations and/or changes.  Once you satisfy all these, yuo pass the code
review.  After you are done witch each set of changes, you need to ping the
reviewer on GitHub to ask for another round of code review until they approve
your changes.

### Interactive grading

After passing the code reviews, you have to go through an interactive grading
session.  This will be a 30-minute interview (i.e., oral exam) where you (1)
explain how your code works, the design decisions you made, and what approaches
you tried have not worked and (2) do a live demo of your assignment.

If you fail interactive grading once, you can do it again with me.  If you fail
it twice, you fail the whole assignment.  The objective of the assignment is to
show that **you have developed a certain understanding** and interactive
grading is where you will be able to demonstrate that.

#### General interactive grading questions

We will ask how you implemented specific features, and different big-picture
design decisions for each feature (what other libraries, database design,
etc. you could have used).
