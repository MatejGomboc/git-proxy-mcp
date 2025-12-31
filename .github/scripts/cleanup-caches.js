/**
 * GitHub Actions Cache Cleanup Script
 *
 * Simple cleanup based on last access time.
 * With the new caching strategy, PRs never create caches and main only
 * saves on tooling changes, so there's minimal cleanup needed.
 *
 * This is mainly a manual utility for proactive cleanup.
 */

module.exports = async ({ github, context, core }) => {
    const config = {
        staleDays: parseInt(process.env.INPUT_STALE_DAYS || '7', 10),
        dryRun: process.env.INPUT_DRY_RUN === 'true',
    };

    const owner = context.repo.owner;
    const repo = context.repo.repo;

    const stats = {
        totalCaches: 0,
        deletedCaches: 0,
        freedBytes: 0,
        errors: 0,
    };

    function formatBytes(bytes) {
        if (bytes === 0) return '0 B';
        const k = 1024;
        const sizes = ['B', 'KB', 'MB', 'GB'];
        const i = Math.floor(Math.log(bytes) / Math.log(k));
        return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
    }

    function getDaysSinceAccess(cache) {
        const lastAccessed = new Date(cache.last_accessed_at || cache.created_at);
        const now = new Date();
        return Math.floor((now - lastAccessed) / (1000 * 60 * 60 * 24));
    }

    function log(message, level = 'info') {
        const prefix = config.dryRun ? '[DRY-RUN] ' : '';
        if (level === 'error') {
            core.error(`${prefix}${message}`);
        } else if (level === 'warning') {
            core.warning(`${prefix}${message}`);
        } else {
            core.info(`${prefix}${message}`);
        }
    }

    async function listAllCaches() {
        const caches = [];
        let page = 1;

        while (true) {
            try {
                const response = await github.rest.actions.getActionsCacheList({
                    owner,
                    repo,
                    per_page: 100,
                    page,
                });

                if (response.data.actions_caches.length === 0) break;
                caches.push(...response.data.actions_caches);
                if (response.data.actions_caches.length < 100) break;
                page++;
            } catch (error) {
                if (error.status === 404) {
                    log('No caches found', 'warning');
                    break;
                }
                throw error;
            }
        }

        return caches;
    }

    async function deleteCache(cache, reason) {
        if (config.dryRun) {
            log(`  Would delete: ${cache.key} (${formatBytes(cache.size_in_bytes)}) [${reason}]`);
            stats.freedBytes += cache.size_in_bytes;
            return true;
        }

        try {
            await github.rest.actions.deleteActionsCacheById({
                owner,
                repo,
                cache_id: cache.id,
            });
            log(`  Deleted: ${cache.key} (${formatBytes(cache.size_in_bytes)}) [${reason}]`);
            stats.freedBytes += cache.size_in_bytes;
            return true;
        } catch (error) {
            log(`  Failed to delete ${cache.key}: ${error.message}`, 'error');
            stats.errors++;
            return false;
        }
    }

    try {
        log('='.repeat(60));
        log('GitHub Actions Cache Cleanup');
        log('='.repeat(60));
        log('');
        log(`Repository:   ${owner}/${repo}`);
        log(`Stale days:   ${config.staleDays}${config.staleDays === 0 ? ' (delete all)' : ''}`);
        log(`Dry run:      ${config.dryRun}`);
        log('');

        const caches = await listAllCaches();
        stats.totalCaches = caches.length;

        if (caches.length === 0) {
            log('No caches found.');
            return;
        }

        log(`Found ${caches.length} cache(s).`);
        log('');

        for (const cache of caches) {
            const daysSinceAccess = getDaysSinceAccess(cache);

            // Delete if stale_days is 0 (all) or cache exceeds threshold
            if (config.staleDays === 0 || daysSinceAccess >= config.staleDays) {
                if (await deleteCache(cache, `${daysSinceAccess}d since last access`)) {
                    stats.deletedCaches++;
                }
            }
        }

        log('');
        log('='.repeat(60));
        log('Summary');
        log('='.repeat(60));
        log(`  Total caches:  ${stats.totalCaches}`);
        log(`  Deleted:       ${stats.deletedCaches}`);
        log(`  Kept:          ${stats.totalCaches - stats.deletedCaches}`);
        log(`  Space freed:   ${formatBytes(stats.freedBytes)}`);

        if (stats.errors > 0) {
            log(`  Errors:        ${stats.errors}`, 'warning');
        }

        if (config.dryRun) {
            log('');
            log('Dry run - no caches were actually deleted.', 'warning');
        }

        core.setOutput('deleted_count', stats.deletedCaches);
        core.setOutput('freed_bytes', stats.freedBytes);

        log('');
        log('✅ Cleanup completed');

    } catch (error) {
        core.setFailed(`Cleanup failed: ${error.message}`);
        throw error;
    }
};
