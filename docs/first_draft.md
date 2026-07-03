### 1. Die wichtigsten Erkenntnisse des Chats

* **PWA statt nativer App:** Eine Progressive Web App (PWA) ist schneller entwickelt, umgeht die App Stores, ist plattformunabhängig und dank Homescreen-Icon und Mikrofon-Zugriff genauso schnell einsatzbereit wie eine native Android-App.
* **Pragmatischer Tech-Stack (Rust + TS):** Das Backend wird für maximale Performance und minimalen RAM-Verbrauch in purem **Rust** geschrieben. Das Frontend wird in **TypeScript (React/Svelte)** umgesetzt, um komplexe 3D-Graphen und Web-APIs ohne umständliche WebAssembly-Brücken schnell einzubinden.
* **LightRAG statt Neo4j:** Eine klobige Graph-Datenbank wie Neo4j ist Overkill. Wir nutzen den modernen "LightRAG"-Ansatz: Ein leichtgewichtiger In-Memory-Graph kombiniert mit einer schnellen Vektordatenbank (Qdrant).
* **GraphRAG & Chat:** Das System ist nicht nur ein visuelles Tagebuch, sondern ein interaktiver Assistent. Du kannst mit deinen eigenen Gedanken chatten, wobei die KI deine lokal verknüpften Knoten als Kontext nutzt (Retrieval-Augmented Generation).
* **Hybride Eingabe:** Der Fokus liegt auf "Voice-First" für spontane Ideen, ergänzt durch ein Textfeld für stille Umgebungen oder schnelle Korrekturen.

---

### 2. Die finale Architektur & Komponenten im Detail

Das System basiert auf einer serviceorientierten Architektur, verpackt in Docker-Containern und organisiert in einem Monorepo mit automatisierten CI/CD-Pipelines.

#### A. Das Frontend (Die Benutzeroberfläche & PWA)

* **Technologie:** TypeScript mit React, Vue oder SvelteKit.
* **Speech-to-Text:** Nutzung der nativen **Web Speech API** des Browsers. Ein riesiger "Record"-Button nimmt die Stimme auf und wandelt sie in Echtzeit in Text um, der in ein hybrides Textfeld zur Endkontrolle fließt.
* **Visualisierung (`3d-force-graph`):** Eine JavaScript-Bibliothek, die das Herzstück der App bildet. Sie rendert deine Gedanken als leuchtende Punkte im 3D-Raum, die durch physikalische Kräfte (Abstoßung/Anziehung) organische Cluster bilden.
* **State & Auth:** Ein einfaches biometrisches Login oder Master-Passwort generiert beim ersten Start ein **Long-Lived JWT (JSON Web Token)**. Dieses wird im `localStorage` des Handys gespeichert, sodass du ab dann sofort und ohne Verzögerung in der App bist.
* **Logging-Dashboard:** Ein versteckter Admin-Tab im Frontend zieht sich über einen API-Endpunkt die System-Logs des Backends, damit du Fehler (z.B. bei der KI-Generierung) direkt am Handy debuggen kannst.

#### B. Das Backend (Der Orchestrator)

* **Technologie:** **Rust** (mit dem Framework *Axum*). Es ist rasend schnell, speichereffizient und bietet höchste Typsicherheit.
* **Aufgabe:** Das Backend ist die Schaltzentrale. Es nimmt den Text vom Frontend entgegen und koordiniert die KI-Aufrufe.
* **Graph-Engine (LightRAG-Logik):** Statt einer externen Graph-Datenbank nutzt das Rust-Backend Bibliotheken wie `petgraph` oder Kùzu, um die Knoten und Kanten direkt im Speicher oder in schlanken lokalen Dateien zu verwalten und für das Frontend in ein JSON-Format zu übersetzen.

#### C. Die KI- & Daten-Pipeline (Das Gehirn)

* **Information Extraction (LLM):** Das Rust-Backend sendet deinen transkribierten Text mit einem strengen System-Prompt an ein LLM (z. B. OpenAI GPT-4o-mini oder Gemini API). Das Modell extrahiert die Entitäten und deren Beziehungen und gibt strukturiertes JSON zurück.
* **Vektordatenbank (Qdrant):** Eine hochperformante, **in Rust geschriebene** Vektordatenbank. Sie läuft als eigener Container und speichert die Embeddings (Vektoren) deiner Gedanken.
* **Embedding-Modell:** Ein lokal gehostetes Open-Source-Modell (z. B. `intfloat/multilingual-e5-large` bereitgestellt durch einen **Ollama**-Container), das auf Deutsch optimiert ist. Es wandelt jeden Knoten in Zahlenreihen (Vektoren) um, damit das System bei Suchanfragen (GraphRAG) semantisch ähnliche Konzepte finden kann.

#### D. Infrastruktur & Deployment (Das Produktions-Setup)

* **Monorepo:** Ein einziges Git-Repository mit Unterordnern für `/frontend`, `/backend` und `/infrastructure`.
* **Docker Compose:** Alle Komponenten (Frontend-Host, Rust-API, Qdrant, Ollama) werden lokal und auf dem Server als isolierte Container orchestriert.
* **Reverse Proxy (Traefik / Nginx Proxy Manager):** Der "Türsteher" deines Servers. Er nimmt Web-Anfragen an, verteilt sie auf Frontend/Backend und kümmert sich vollautomatisch um **SSL-Zertifikate (HTTPS)**. *Wichtig: Ohne HTTPS blockiert der Handy-Browser das Mikrofon!*
* **Hosting:** Ein Virtual Private Server (VPS), z. B. bei Hetzner (Cloud).
* **CI/CD (GitHub Actions):** Sobald du Code-Änderungen in den `main`-Branch pushst, läuft ein Script los, das den Code testet, die Docker-Images neu baut und sie ohne manuelles Eingreifen auf deinen Server hochlädt.
