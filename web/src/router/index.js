import { createRouter, createWebHashHistory } from "vue-router";
import InboxView from "@/views/InboxView.vue";

const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    {
      path: "/",
      name: "chat",
      component: InboxView
    },
    {
      path: "/chat",
      redirect: "/"
    }
  ]
});

export default router;
