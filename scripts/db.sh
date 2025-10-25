#!/bin/bash
# Database helper script for containerized PostgreSQL
# This script provides easy access to PostgreSQL commands when running in Docker

set -e

# Determine which docker-compose file to use
COMPOSE_FILE="docker-compose.local.yml"
CONTAINER_NAME="aiwebengine-postgres-dev"
DB_NAME="aiwebengine"
DB_USER="aiwebengine"

# Check if production mode
if [ "$1" = "--prod" ] || [ "$1" = "-p" ]; then
    COMPOSE_FILE="docker-compose.yml"
    CONTAINER_NAME="aiwebengine-postgres"
    shift
fi

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Check if postgres container is running
check_container() {
    if ! docker ps --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
        echo -e "${RED}Error: PostgreSQL container '${CONTAINER_NAME}' is not running${NC}"
        echo -e "${YELLOW}Start it with: docker-compose -f ${COMPOSE_FILE} up -d postgres${NC}"
        exit 1
    fi
}

# Show usage
usage() {
    echo "Usage: $0 [--prod|-p] <command> [args...]"
    echo ""
    echo "Commands:"
    echo "  psql [args]         - Run psql (PostgreSQL interactive terminal)"
    echo "  createdb [name]     - Create a database (default: aiwebengine)"
    echo "  dropdb [name]       - Drop a database (default: aiwebengine)"
    echo "  migrate-run         - Run SQLx migrations"
    echo "  migrate-revert      - Revert last migration"
    echo "  migrate-info        - Show migration status"
    echo "  backup [file]       - Backup database to file"
    echo "  restore <file>      - Restore database from file"
    echo "  shell               - Open a shell in the postgres container"
    echo "  logs                - Show postgres container logs"
    echo ""
    echo "Options:"
    echo "  --prod, -p          - Use production containers (default: local/dev)"
    echo ""
    echo "Examples:"
    echo "  $0 psql                          # Interactive psql"
    echo "  $0 psql -c 'SELECT * FROM users' # Run SQL query"
    echo "  $0 createdb                      # Create aiwebengine database"
    echo "  $0 migrate-run                   # Run migrations"
    echo "  $0 backup backup.sql             # Backup database"
    echo "  $0 --prod migrate-run            # Run migrations in production"
}

# Main command router
case "${1:-}" in
    psql)
        check_container
        shift
        echo -e "${GREEN}Connecting to PostgreSQL...${NC}"
        docker exec -it "${CONTAINER_NAME}" psql -U "${DB_USER}" -d "${DB_NAME}" "$@"
        ;;
    
    createdb)
        check_container
        TARGET_DB="${2:-${DB_NAME}}"
        echo -e "${GREEN}Creating database: ${TARGET_DB}${NC}"
        docker exec -it "${CONTAINER_NAME}" createdb -U "${DB_USER}" "${TARGET_DB}"
        echo -e "${GREEN}Database '${TARGET_DB}' created successfully${NC}"
        ;;
    
    dropdb)
        check_container
        TARGET_DB="${2:-${DB_NAME}}"
        echo -e "${YELLOW}Are you sure you want to drop database '${TARGET_DB}'? (yes/no)${NC}"
        read -r confirm
        if [ "$confirm" = "yes" ]; then
            echo -e "${GREEN}Dropping database: ${TARGET_DB}${NC}"
            docker exec -it "${CONTAINER_NAME}" dropdb -U "${DB_USER}" "${TARGET_DB}"
            echo -e "${GREEN}Database '${TARGET_DB}' dropped${NC}"
        else
            echo -e "${YELLOW}Cancelled${NC}"
        fi
        ;;
    
    migrate-run)
        check_container
        echo -e "${GREEN}Running SQLx migrations...${NC}"
        # Set DATABASE_URL based on environment
        if [ "${COMPOSE_FILE}" = "docker-compose.yml" ]; then
            export DATABASE_URL="postgresql://${DB_USER}:${POSTGRES_PASSWORD:-change-this-password}@localhost:5432/${DB_NAME}"
        else
            export DATABASE_URL="postgresql://${DB_USER}:devpassword@localhost:5432/${DB_NAME}"
        fi
        sqlx migrate run
        echo -e "${GREEN}Migrations completed${NC}"
        ;;
    
    migrate-revert)
        check_container
        echo -e "${YELLOW}Reverting last migration...${NC}"
        if [ "${COMPOSE_FILE}" = "docker-compose.yml" ]; then
            export DATABASE_URL="postgresql://${DB_USER}:${POSTGRES_PASSWORD:-change-this-password}@localhost:5432/${DB_NAME}"
        else
            export DATABASE_URL="postgresql://${DB_USER}:devpassword@localhost:5432/${DB_NAME}"
        fi
        sqlx migrate revert
        echo -e "${GREEN}Migration reverted${NC}"
        ;;
    
    migrate-info)
        check_container
        echo -e "${GREEN}Migration status:${NC}"
        if [ "${COMPOSE_FILE}" = "docker-compose.yml" ]; then
            export DATABASE_URL="postgresql://${DB_USER}:${POSTGRES_PASSWORD:-change-this-password}@localhost:5432/${DB_NAME}"
        else
            export DATABASE_URL="postgresql://${DB_USER}:devpassword@localhost:5432/${DB_NAME}"
        fi
        sqlx migrate info
        ;;
    
    backup)
        check_container
        BACKUP_FILE="${2:-backup_$(date +%Y%m%d_%H%M%S).sql}"
        echo -e "${GREEN}Backing up database to: ${BACKUP_FILE}${NC}"
        docker exec -t "${CONTAINER_NAME}" pg_dump -U "${DB_USER}" "${DB_NAME}" > "${BACKUP_FILE}"
        echo -e "${GREEN}Backup completed: ${BACKUP_FILE}${NC}"
        ;;
    
    restore)
        if [ -z "$2" ]; then
            echo -e "${RED}Error: Restore file required${NC}"
            echo "Usage: $0 restore <file>"
            exit 1
        fi
        check_container
        RESTORE_FILE="$2"
        if [ ! -f "${RESTORE_FILE}" ]; then
            echo -e "${RED}Error: File not found: ${RESTORE_FILE}${NC}"
            exit 1
        fi
        echo -e "${YELLOW}This will restore from: ${RESTORE_FILE}${NC}"
        echo -e "${YELLOW}Existing data will be lost. Continue? (yes/no)${NC}"
        read -r confirm
        if [ "$confirm" = "yes" ]; then
            echo -e "${GREEN}Restoring database...${NC}"
            docker exec -i "${CONTAINER_NAME}" psql -U "${DB_USER}" -d "${DB_NAME}" < "${RESTORE_FILE}"
            echo -e "${GREEN}Restore completed${NC}"
        else
            echo -e "${YELLOW}Cancelled${NC}"
        fi
        ;;
    
    shell)
        check_container
        echo -e "${GREEN}Opening shell in postgres container...${NC}"
        docker exec -it "${CONTAINER_NAME}" /bin/sh
        ;;
    
    logs)
        echo -e "${GREEN}Showing postgres logs...${NC}"
        docker-compose -f "${COMPOSE_FILE}" logs -f postgres
        ;;
    
    -h|--help|help|"")
        usage
        ;;
    
    *)
        echo -e "${RED}Unknown command: $1${NC}"
        echo ""
        usage
        exit 1
        ;;
esac
