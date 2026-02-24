// This script runs automatically when MongoDB starts with a fresh volume

// Switch to nasiko database
db = db.getSiblingDB('nasiko');

// Create nasiko_admin user with both database-specific and global admin privileges
db.createUser({
  user: "nasiko_admin",
  pwd: "nasiko_password123",
  roles: [
    // Database-specific admin for nasiko
    {
      role: "dbOwner",
      db: "nasiko"
    },
    // Global admin privileges
    {
      role: "userAdminAnyDatabase",
      db: "admin"
    },
    {
      role: "dbAdminAnyDatabase",
      db: "admin"
    },
    {
      role: "readWriteAnyDatabase",
      db: "admin"
    },
    {
      role: "clusterAdmin",
      db: "admin"
    }
  ]
});

print("Database and user setup completed successfully!");