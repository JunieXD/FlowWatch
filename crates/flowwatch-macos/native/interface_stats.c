#include <errno.h>
#include <stdint.h>
#include <string.h>
#include <sys/sysctl.h>
#include <sys/socket.h>
#include <net/if.h>
#include <net/if_mib.h>
#include <net/if_var.h>

struct flowwatch_interface_counter {
    char name[IF_NAMESIZE];
    uint64_t upload;
    uint64_t download;
};

int flowwatch_interface_counters(
    struct flowwatch_interface_counter *output,
    size_t capacity,
    size_t *written
) {
    if (written == NULL) {
        return EINVAL;
    }
    *written = 0;
    struct if_nameindex *interfaces = if_nameindex();
    if (interfaces == NULL) {
        return errno;
    }
    for (struct if_nameindex *item = interfaces;
         item->if_index != 0 && item->if_name != NULL; ++item) {
        int mib[6] = {CTL_NET, PF_LINK, NETLINK_GENERIC,
                      IFMIB_IFDATA, (int)item->if_index, IFDATA_GENERAL};
        struct ifmibdata data;
        size_t length = sizeof(data);
        memset(&data, 0, sizeof(data));
        if (sysctl(mib, 6, &data, &length, NULL, 0) != 0) {
            continue;
        }
        if (*written >= capacity) {
            if_freenameindex(interfaces);
            return ENOBUFS;
        }
        strlcpy(output[*written].name, item->if_name, IF_NAMESIZE);
        output[*written].upload = data.ifmd_data.ifi_obytes;
        output[*written].download = data.ifmd_data.ifi_ibytes;
        *written += 1;
    }
    if_freenameindex(interfaces);
    return 0;
}
