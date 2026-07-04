# First Draft — Second Brain (überarbeitet, Stand Juli 2026)

_Dieser Draft wurde gegen aktuelle Primärquellen (Stand Juli 2026) auf den bestmöglichen, modernsten Stack für diesen Anwendungsfall geprüft: Single-User-PWA, Deutsch als Primärsprache, 8-GB-Hetzner-VPS, gehostete KI-APIs (kein Self-Hosting von Modellen). Geänderte Entscheidungen sind im Text und in der Änderungsliste unten begründet._

## Was sich seit dem Ursprungs-Draft geändert hat

- ~~Ollama + `intfloat/multilingual-e5-large`~~ → **Cohere `embed-v4.0`** (gehostete multilinguale Embedding-API, Deutsch first-class). Self-Hosting eines Embedding-Modells auf einem 8-GB-VPS ist für eine Single-User-App reiner Overhead.
- ~~Qdrant (eigener Container)~~ → **sqlite-vec (in-process, im selben SQLite wie der Graph)**. Ein separater Vector-DB-Server ist bei Personal-Scale falsch und bricht die Ingest-Transaktion mit der Concept-Identity-Merge (ADR-0001).
- ~~petgraph oder Kùzu~~ → **petgraph (in-memory) + SQLite (`rusqlite`)** für ACID-Persistenz. Kùzu wurde im Oktober 2025 archiviert/eingefroren — keine Option mehr für einen wachsenden, load-bearing Graph.
- ~~Web Speech API als primärer STT-Pfad~~ → **Deepgram Nova-3** (streaming, Deutsch). Web Speech API nur noch Offline-Fallback.
- ~~"React, Vue oder SvelteKit" (unentschieden)~~ → **SvelteKit (Svelte 5)**. Die Unentschiedenheit fror alle Abhängigkeiten ein.
- ~~JWT in `localStorage`~~ → **Passkey (WebAuthn) + `httpOnly; Secure; SameSite=Strict` Session-Cookie**. JWT-in-localStorage ist ein XSS-Anti-Pattern und nicht widerrufbar.
- ~~Traefik / Nginx Proxy Manager~~ → **Caddy** (auto-HTTPS by default, HTTP/3 by default, ~5-Zeilen-Caddyfile).
- **`3d-force-graph` bleibt**, bekommt aber **graphology als Datenmodell** und **sigma.js als 2D-Mobil-Fallback**.
- **Axum, PWA, Docker Compose, Hetzner, GitHub Actions** bleiben — bestätigt als weiterhin bestmögliche Wahl.

---

### 1. Die wichtigsten Erkenntnisse

* **PWA statt nativer App:** Eine Progressive Web App (PWA) ist schneller entwickelt, umgeht die App Stores, ist plattformunabhängig und dank Homescreen-Icon und Mikrofon-Zugriff genauso schnell einsatzbereit wie eine native Android-App. Capacitor bleibt als dokumentierter Escape-Hatch, falls je ein natives API nötig wird — für Single-User + Voice-First + Homescreen-Icon ist die PWA genau der Sweet Spot.
* **Pragmatischer Tech-Stack (Rust + TS):** Das Backend in purem **Rust (Axum)** für maximale Performance und Typsicherheit; das Frontend in **TypeScript mit SvelteKit (Svelte 5)** — kleinster Bundle der Kandidaten = bester mobiler PWA-Kaltstart, und da `3d-force-graph` framework-agnostisch ist, entfällt Reacts Ökosystem-Vorteil hier.
* **Custom Rust Graph statt LightRAG-Library statt Neo4j:** Die LightRAG-Idee (Graph load-bearing, Vektoren als Seed + Backfill, ADR-0004) wird als **eigene Rust-Implementierung** umgesetzt — 2026 gibt es keine Rust-LightRAG (alle reifen Optionen — LightRAG, MS GraphRAG, nano-GraphRAG, fast-graphrag — sind Python), und das Projekt-Modell (typisierte Kanten, origin-typed Provenance, governed Ontology, event-sourced Type-History) ist bewusst strikter als LightRAG. Statt Neo4j oder Kùzu: **petgraph in-memory + SQLite** für ACID-Persistenz.
* **GraphRAG & Chat:** Das System ist nicht nur ein visuelles Tagebuch, sondern ein interaktiver Assistent. Du chattest mit deinen eigenen Gedanken, wobei die KI deine lokal verknüpften Knoten als Kontext nutzt (Retrieval-Augmented Generation, ADR-0004/0005).
* **Hybride Eingabe (Voice-First):** Der Fokus liegt auf Voice-First für spontane Ideen, ergänzt durch ein Textfeld für stille Umgebungen oder schnelle Korrekturen. Primärer STT-Pfad ist **Deepgram Nova-3** (streaming, Deutsch-erstklassig) — die Transkriptionsqualität füttert direkt die LLM-Extraktions-Pipeline.
* **Gehostete KI-APIs statt Self-Hosting:** LLM (Extraktion + Chat) und Embeddings laufen über gehostete APIs — kein Ollama, kein self-gehostetes Modell. Das hält den VPS schlank und die Qualität deterministisch.

---

### 2. Die finale Architektur & Komponenten im Detail

Das System basiert auf einer serviceorientierten Architektur, verpackt in Docker-Containern und organisiert in einem Monorepo mit automatisierten CI/CD-Pipelines.

#### A. Das Frontend (Die Benutzeroberfläche & PWA)

* **Technologie:** TypeScript mit **SvelteKit (Svelte 5)**. Kleinster Bundle der Kandidaten, beste DX; `3d-force-graph` ist ein framework-agnostisches Web-Component, darum ist Reacts `react-force-graph`-Vorteil hier hinfällig. Capacitor als dokumentierter Escape-Hatch für künftige native API-Bedürfnisse.
* **Speech-to-Text:** Primär **Deepgram Nova-3** (streaming, Deutsch-erstklassig, prompt-basierte Korrektur von Namen/Jargon) — die Transkriptionsqualität füttert direkt die LLM-Extraktions-Pipeline, darum zählen Qualität und Latenz mehr als "kostenlos". **Web Speech API** nur noch als Offline-Fallback. Budget-Alternative: **Groq `whisper-large-v3`** (≈⅓ des Preises, nahezu gleiche Deutsch-Qualität).
* **Visualisierung:** **`3d-force-graph`** (ThreeJS/WebGL) als 3D-Herzstück mit Bloom-Postprocessing für die "leuchtenden Punkte" und physikalischer Cluster-Bildung. **graphology** ab Tag 1 als Datenmodell (ForceAtlas2-Layout, Louvain-Community-Detection = organische Cluster) — unabhängig vom Renderer. **sigma.js (2D WebGL)** als dokumentierter Mobil-Fallback, falls 3D-WebGL auf Mid-Range-Android zu träge ist; graphology+sigma liefert dasselbe Cluster-Gefühl in 2D mit deutlich besserer Mobil-Performance.
* **Auth:** **Passkey (WebAuthn)** als primärer Faktor — Android-Fingerprint/Face, phishing-resistent, passwordless. Beim Login mintet der Server eine opake Session-ID (≥64-Bit, CSPRNG) als `httpOnly; Secure; SameSite=Strict` (ideal `__Host-`-präfixiertes) Cookie, mit dem echten Session-State server-seitig in einer SQLite-Zeile. Master-Passphrase nur als Wiederherstellungsweg. ~~JWT in `localStorage`~~ — XSS-verwundbar und nicht widerrufbar (OWASP warnt explizit davor).
* **Logging-Dashboard:** Ein versteckter Admin-Tab im Frontend zieht sich über einen API-Endpunkt die System-Logs des Backends, damit du Fehler (z.B. bei der KI-Generierung) direkt am Handy debuggen kannst.

#### B. Das Backend (Der Orchestrator)

* **Technologie:** **Rust mit Axum** (v0.8.x). Baut auf `tower`/`hyper`/`tokio` — Retries/Timeouts/Backpressure (`tower::ServiceBuilder`) und der HTTP-Client (`reqwest`) teilen sich eine Runtime und ein Middleware-Vokabular. Rasend schnell, speichereffizient, höchste Typsicherheit; 2026 weiterhin die beste Wahl für einen asynchronen Orchestrator.
* **Aufgabe:** Das Backend ist die Schaltzentrale. Es nimmt den Text vom Frontend entgegen und koordiniert die KI-Aufrufe (Extraktion, Embeddings, Retrieval, Chat).
* **Graph-Engine:** **petgraph (in-memory) + SQLite via `rusqlite`** für ACID-Persistenz. Die typisierten Kanten, Provenance-Listen und die append-only Type-History (event-sourced, ADR-0003) sind natürliche Zeilen in typisierten Tabellen; SQLite liefert die transaktionale Sicherheit, die "schlanke lokale Datei" im Ursprungs-Draft unterschlug. Beim Start wird der Graph aus dem Event-Log in-memory rehydratisiert. ~~Kùzu~~ wurde im Oktober 2025 archiviert (v0.11.3, letzte Version) — ausgeschieden. Kein Neo4j, kein externer Graph-DB-Server.
* **Vector-Store:** **sqlite-vec (in-process)** — die Vektoren leben im selben SQLite wie der Graph, darum ist die Concept-Identity-Merge (Embedding-Match → Insert/Merge, ADR-0001) eine atomare Transaktion statt eines Netzwerk-Hops. Bei Personal-Scale ist sogar Brute-Force-KNN sub-ms. ~~Qdrant (Container oder Cloud)~~ — ops-Overhead für ~MB Daten und bricht die Ingest-Transaktion. Die drei Embedding-Collections (braindump/concept/type, ADR-0003) leben alle in-process.

#### C. Die KI- & Daten-Pipeline (Das Gehirn)

* **Information Extraction (LLM):** Gehostete API (Gemini) mit strengem System-Prompt → strukturiertes JSON mit Entitäten und typisierten Relationen. Für deterministische Ontology-Refactors (ADR-0003): Temperature=0 gegen einen **gepinnten Model-Snapshot**, nie `-latest` — so retagt der Hintergrund-Job stabil über API-Model-Bumps hinweg.
* **Embeddings:** **auch gemini,
* ~~**Vektordatenbank (Qdrant)** + **Embedding-Modell (Ollama)**~~ → entfallen als eigene Container; siehe B. Vector-Store und C. Embeddings. Die Pipeline ist LLM-API → Embedding-API → in-process SQLite/sqlite-vec; kein zweiter Daten-Server.
* **LightRAG-Ansatz:** Die Idee (Graph load-bearing, Vektoren Seed+Backfill, ADR-0004) wird als **eigene Rust-Implementierung** umgesetzt. 2026 gibt es keine Rust-LightRAG; die reifen Optionen (LightRAG HKUDS, MS GraphRAG, nano-GraphRAG, fast-graphrag) sind alle Python — eine Adoption hieße einen Python-Sidecar oder einen Port, und das Projekt-Modell (typisierte Kanten, origin-typed Provenance, governed Ontology, event-sourced Type-History) ist bewusst strikter als LightRAG. Wir borgen uns LightRAGs Retrieval-Algorithmus, nicht seine Bibliothek.

#### D. Infrastruktur & Deployment (Das Produktions-Setup)

* **Monorepo:** Ein einziges Git-Repository mit Unterordnern für `/frontend`, `/backend` und `/infrastructure`.
* **Docker Compose:** orchestriert Frontend-Host und Rust-API; die Daten liegen in SQLite in-process — ~~kein Qdrant-Container, kein Ollama-Container~~ mehr. Deutlich schlankeres Compose als im Ursprungs-Draft.
* **Reverse Proxy:** **Caddy** — auto-HTTPS (Let's Encrypt/ZeroSSL) by default, HTTP/3 by default, ~5-Zeilen-Caddyfile, ~30–40 MB RSS. *Wichtig: Ohne HTTPS blockiert der Handy-Browser das Mikrofon — Caddy erledigt das automatisch.* ~~Traefik / Nginx Proxy Manager~~ — für einen statischen Single-Backend Overkill. *(Falls später ein PaaS wie Coolify/Dokploy eingesetzt wird, dessen gebündeltes Traefik übernehmen — keinen zweiten Proxy betreiben.)*
* **Hosting:** Ein Hetzner VPS mit 8 GB RAM — komfortabel für Single-User, ausreichend für Graph/Vektoren in SQLite und die API-Aufrufe.
* **CI/CD (GitHub Actions):** Push auf `main` → Code testen, Docker-Images bauen, per SSH `docker compose pull && up -d` auf den Server. Optional Coolify/Dokploy für einen git-push-to-deploy-PaaS-Flow (inkl. PR-Preview-Deploys, Backups, Monitoring) — dann entfällt das eigene Deploy-Script.
