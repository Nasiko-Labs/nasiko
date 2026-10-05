export interface SampleResume {
  id: string;
  name: string;
  role: string;
  summary: string;
  skillsPreview: string[];
  resumeText: string;
}

export const SAMPLE_RESUMES: SampleResume[] = [
  {
    id: 'java-spring-boot',
    name: 'Alex Rivera',
    role: 'Java Backend Developer',
    summary: 'Senior Java Backend Engineer with 4 years building Spring Boot microservices, REST APIs, MySQL persistence, and Docker deployments.',
    skillsPreview: ['Java 17', 'Spring Boot 3', 'Spring Security', 'MySQL', 'Docker', 'AWS', 'Microservices'],
    resumeText: `ALEX RIVERA
Email: alex.rivera.dev@gmail.com | Phone: +1 (555) 432-8765 | Location: Austin, TX
LinkedIn: linkedin.com/in/alex-rivera-tech | GitHub: github.com/arivera-code

PROFESSIONAL SUMMARY
Results-driven Java Backend Developer with 4 years of experience specializing in high-throughput REST APIs, Spring Boot microservice architectures, and relational database tuning. Demonstrated track record in modernizing legacy monoliths into distributed Dockerized services on AWS.

EDUCATION
Bachelor of Science in Computer Science
University of Texas at Austin (2019 - 2023) | CGPA: 3.82 / 4.0

TECHNICAL SKILLS
- Programming Languages: Java (8/11/17), SQL, Bash
- Backend Frameworks: Spring Boot, Spring MVC, Spring Data JPA, Hibernate, Spring Security
- Databases & Caching: MySQL, PostgreSQL, Redis, Flyway
- Cloud & DevOps: Docker, Kubernetes, AWS (EC2, S3, RDS), Jenkins CI/CD, Git
- Testing: JUnit 5, Mockito, Testcontainers, Postman
- Concepts & Architecture: Microservices, RESTful APIs, Event-Driven Architecture, JWT, ACID Transactions

PROFESSIONAL EXPERIENCE
Software Engineer | Apex FinTech Solutions | Austin, TX
July 2023 - Present
- Designed and maintained scalable Spring Boot microservices handling over 5,000 requests/sec for real-time transaction processing.
- Optimized slow MySQL queries and indexed database tables, decreasing p99 response latencies by 38%.
- Implemented JWT token validation and role-based access control filters using Spring Security.
- Containerized applications using Docker and configured continuous deployment pipelines with Jenkins and AWS ECS.

Junior Software Engineer | CloudVenture Systems | Austin, TX
June 2021 - June 2023
- Built and documented RESTful API endpoints for client account provisioning and reporting modules.
- Refactored legacy JDBC queries into Spring Data JPA repositories, reducing boilerplate data access code by 45%.
- Wrote comprehensive unit and integration tests using JUnit 5 and Mockito, achieving 88% code test coverage.

KEY PROJECTS
Employee Management System & Payroll Engine
Technologies: Java 17, Spring Boot, Spring Security, MySQL, Docker
- Architected a multi-tenant employee management platform with attendance tracking and automated payroll calculation.
- Implemented secure authentication with JWT bearer tokens, refresh tokens, and BCrypt password encryption.
- Built automated background scheduling with Spring @Scheduled for end-of-month compensation audits.

Distributed Order Processing Gateway
Technologies: Spring Boot, Apache Kafka, Redis, PostgreSQL, AWS
- Implemented an asynchronous order validation service using Kafka event streams to decouple checkout from inventory.
- Utilized Redis distributed caching for hot product catalogs, reducing relational database load by 50%.

CERTIFICATIONS
- AWS Certified Solutions Architect - Associate (2024)
- Oracle Certified Professional: Java SE 11 Developer (2023)
`,
  },
  {
    id: 'fullstack-react-node',
    name: 'Maya Chen',
    role: 'Full Stack TypeScript Engineer',
    summary: 'Full Stack Engineer with 3+ years experience building reactive frontend dashboards in React/TypeScript and distributed Node.js/Express backends.',
    skillsPreview: ['TypeScript', 'React', 'Node.js', 'Express', 'Tailwind CSS', 'PostgreSQL', 'Redis'],
    resumeText: `MAYA CHEN
Email: maya.chen.dev@outlook.com | Phone: +1 (555) 789-1234 | Location: San Francisco, CA
Portfolio: mayachen.dev | GitHub: github.com/mayachen-ui

PROFESSIONAL SUMMARY
Full Stack Software Engineer with expertise in modern TypeScript ecosystems, component-driven React design systems, and robust Node.js backend services. Passionate about web performance, clean state management, and real-time collaborative applications.

EDUCATION
B.S. in Software Engineering
San Jose State University (2020 - 2024) | GPA: 3.75

TECHNICAL SKILLS
- Languages: TypeScript, JavaScript (ES6+), Python, HTML5, CSS3
- Frontend: React 18, Next.js, Tailwind CSS, Redux Toolkit, React Query, WebSockets
- Backend: Node.js, Express.js, NestJS, GraphQL, REST APIs
- Databases: PostgreSQL, MongoDB, Redis, Prisma ORM
- DevOps & Tools: Docker, Git, GitHub Actions, Vercel, Vite
- Testing: Vitest, Jest, Playwright, React Testing Library

EXPERIENCE
Full Stack Developer | NexaStream Analytics | San Francisco, CA
August 2023 - Present
- Engineered responsive analytical dashboard in React 18 with data visualizations using Recharts and WebSockets.
- Created Express.js API microservices with Prisma ORM and PostgreSQL handling real-time telemetry streaming.
- Reduced initial frontend bundle load times by 42% through code-splitting, tree-shaking, and asset caching.

Frontend Engineering Intern | PulseMedia Labs | Mountain View, CA
May 2022 - August 2022
- Developed accessible UI component library adhering to W3C WCAG 2.1 AA guidelines.
- Integrated payment gateway endpoints using Stripe Webhooks and secure client-side tokens.

PROJECTS
Real-Time Collaborative Whiteboard
Technologies: TypeScript, React, Node.js, Socket.io, Canvas API
- Created an interactive canvas where up to 50 concurrent users can draw, leave sticky notes, and export SVGs.
- Implemented operational transform algorithms to resolve concurrent drawing state conflicts.

CERTIFICATIONS
- Meta Certified Front-End Developer (2023)
`,
  },
  {
    id: 'python-cloud-backend',
    name: 'Devon Vance',
    role: 'Python & Cloud Backend Engineer',
    summary: 'Backend Engineer specializing in Python, FastAPI, asynchronous event queues, and cloud infrastructure on GCP and AWS.',
    skillsPreview: ['Python 3.11', 'FastAPI', 'PostgreSQL', 'Celery', 'Redis', 'Docker', 'GCP'],
    resumeText: `DEVON VANCE
Email: devon.vance@techmail.io | Location: Seattle, WA | GitHub: github.com/dvance-cloud

PROFESSIONAL SUMMARY
Backend Engineer with 3 years building high-concurrency asynchronous API services in Python (FastAPI/AsyncIO), managing distributed background worker queues with Celery/Redis, and maintaining automated CI/CD pipelines.

EDUCATION
B.S. in Computer Engineering | University of Washington (2022)

TECHNICAL SKILLS
- Languages: Python, Go, SQL
- Frameworks: FastAPI, Flask, Pydantic, SQLAlchemy, Celery
- Databases: PostgreSQL, Redis, Elasticsearch
- Cloud & Infrastructure: Google Cloud Platform (Cloud Run, Cloud SQL), AWS, Docker, Terraform
- Testing & Tooling: Pytest, Tox, Ruff, Postman, Git

EXPERIENCE
Backend Software Engineer | DataForge IO | Seattle, WA
September 2022 - Present
- Architected async FastAPI microservices handling ingestion of 10M daily events into PostgreSQL.
- Implemented background task orchestration using Celery and Redis broker with exponential retry backoff.
- Set up Dockerized container environments deployed on Google Cloud Run with automated GitHub Actions.

PROJECTS
Distributed Web Scraping & Content Summarization Engine
Technologies: Python 3.11, FastAPI, Celery, Redis, Beautiful Soup, OpenAI API
- Built an asynchronous distributed crawler pipeline that extracts article text and generates NLP summaries.
`,
  },
];
